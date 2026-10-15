# 파일 자동 정리 프로그램 — 설계 스펙 (v0.1)

> 가칭: **Tidy** (추후 변경 가능)
> 작성일: 2026-10-15
> 용도: 이 문서는 Claude Code가 구현 기준으로 삼는 단일 소스(Single Source of Truth)다. 구현 중 결정이 바뀌면 이 문서를 먼저 갱신한다.

---

## 1. 목표

바탕화면과 사용자가 지정한 폴더(로컬·NAS 마운트)를 **상시 감시**하면서,

1. 새 파일이 생기면 **규칙 → AI 판단 → 사용자 확인** 순으로 목적지를 정해 이동시키고,
2. 폴더 전체 구조를 **트리/통계 리포트**로 뽑아 재구성안을 제안하며,
3. 파일 메타데이터와 **본문 내용까지 인덱싱**해 즉시 검색을 제공한다.

AI는 선택 사항이다. API 키나 로컬 모델이 없어도 규칙 엔진 + 검색은 완전히 동작해야 한다.

---

## 2. 범위

### 2.1 1차 (MVP)
- 폴더 감시 (로컬 이벤트 + NAS 폴링 하이브리드)
- 규칙 기반 분류 (확장자 / 파일명 패턴 / 출처 / 크기 / 날짜)
- AI 분류 제안 (클라우드 API + Ollama 로컬) — 키 없으면 비활성
- 사용자 확인 UI (트레이 알림 → 승인/수정/건너뜀)
- 이동 실행 + **전체 이동 로그 + 되돌리기(Undo)**
- 인덱싱: 메타데이터 + 텍스트 본문(txt, md, pdf, docx, xlsx, pptx, csv, json, 소스코드, html)
- 전문검색 (SQLite FTS5) + 선택적 의미검색(임베딩)
- 구조 리포트: 트리, 용량, 파일 타입 분포, 중복(해시), 오래된 파일
- AI 재구성 제안 → 미리보기(diff) → 적용/거부
- 트레이 상주, 글로벌 단축키로 검색창
- 학습 루프: 같은 패턴에 같은 답이 N회 쌓이면 자동 규칙 승격 제안

### 2.2 2차 (1차 완성 후)
- 이미지 OCR / 이미지 내용 설명(비전 모델) 인덱싱
- 스캔 PDF OCR
- NAS 측 헤드리스 에이전트(Docker) — NAS가 직접 감시·인덱싱, PC 앱은 결과 동기화
- 멀티 디바이스 인덱스 공유
- 자연어 명령 ("지난달 받은 KT 관련 엑셀 전부 프로젝트 폴더로")

### 2.3 하지 않는 것
- 클라우드 스토리지 API 직접 연동(구글드라이브 등) — 마운트된 폴더로만 취급
- 파일 삭제 자동화 — 삭제는 항상 사용자 명시 조작, 휴지통 경유
- 암호화 파일 본문 해독

---

## 3. 기술 스택

| 영역 | 선택 | 이유 |
|---|---|---|
| 코어 | **Rust** | 파일 감시·해시·텍스트 추출·DB를 한 라이브러리로. NAS 데몬으로 재사용 |
| UI | **Tauri 2** + React + TypeScript | 트레이·글로벌 단축키·네이티브 알림 기본 지원, 가벼움 |
| DB | SQLite (rusqlite, bundled) + FTS5 | 단일 파일, 의존성 없음 |
| 벡터 | sqlite-vec (또는 초기엔 BLOB + 코사인 brute-force) | 개인용 규모에선 충분 |
| 파일 감시 | `notify` crate (+ `notify-debouncer-full`) | 크로스플랫폼 |
| 텍스트 추출 | pdf-extract / docx(zip+xml) / calamine(xlsx) / 자체 파서 | 순수 Rust 우선, 실패 시 외부 CLI 폴백 옵션 |
| LLM | 자체 `LlmProvider` 트레이트 → Anthropic / OpenAI / Ollama 구현 | 교체 가능 |
| 설정 | TOML (`config.toml`) | 사람이 읽고 수정 가능 |
| 로깅 | `tracing` + 파일 롤링 | |

---

## 4. 아키텍처

```
┌─────────────────────────────────────────────────────────┐
│ Tauri App (tray-resident)                               │
│  ┌──────────────┐  ┌───────────────┐  ┌──────────────┐  │
│  │ Search Window│  │ Confirm Popup │  │ Report/Settings│ │
│  └──────┬───────┘  └──────┬────────┘  └──────┬───────┘  │
│         └────────── Tauri IPC (commands/events) ─────┐  │
└──────────────────────────────────────────────────────┼──┘
                                                       │
┌──────────────────────────────────────────────────────┼──┐
│ tidy-core (Rust lib)                                  ▼  │
│  ┌────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │Watcher │→ │Classifier│→ │ Mover    │  │ Reporter │    │
│  │(event+ │  │rules→AI  │  │+UndoLog  │  │tree/dup  │    │
│  │ poll)  │  └────┬─────┘  └──────────┘  └──────────┘    │
│  └───┬────┘       │                                      │
│      ▼            ▼                                      │
│  ┌────────┐  ┌──────────┐  ┌──────────────────────────┐  │
│  │Indexer │  │ AI Layer │  │ Store (SQLite+FTS5+vec)  │  │
│  │extract │  │Provider  │  │                          │  │
│  │+embed  │  │trait     │  │                          │  │
│  └────────┘  └──────────┘  └──────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

**원칙**
- `tidy-core`는 UI를 모른다. 모든 기능은 코어 API로 노출되고, Tauri는 얇은 어댑터다.
- 코어는 `tidy-cli`로도 호출 가능해야 한다(테스트·NAS 데몬 전환 대비).
- 모든 파일시스템 변경은 **Mover**를 통해서만 일어난다. 다른 모듈은 파일을 쓰거나 옮기지 않는다.

### 4.1 워크스페이스 구조
```
tidy/
├── Cargo.toml              # workspace
├── crates/
│   ├── tidy-core/          # 핵심 라이브러리
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs
│   │       ├── engine.rs   # 파일 1건: 분류 → 대기열 또는 Mover (+인덱스 반영) (watch/앱 공용)
│   │       ├── pending.rs  # 대기열 항목 처리: 이동/다른 곳/건너뜀/무시 (CLI·앱 공용)
│   │       ├── service.rs  # 상시 감시 루프 run_watch (CLI `watch`·앱 공용)
│   │       ├── watcher/    # 이벤트 + 폴링
│   │       ├── classify/   # rules.rs, ai.rs, learner.rs
│   │       ├── mover/      # mover.rs, undo.rs
│   │       ├── index/      # extract/*.rs, embed.rs, indexer.rs
│   │       ├── search/     # fts.rs, semantic.rs, query.rs
│   │       ├── report/     # tree.rs, dup.rs, stale.rs, restructure.rs
│   │       ├── ai/         # provider.rs, anthropic.rs, openai.rs, ollama.rs, prompts.rs, keys.rs, mock.rs
│   │       └── store/      # db.rs, migrations/, models.rs
│   └── tidy-cli/           # 디버그/헤드리스용 CLI
├── apps/
│   └── desktop/            # Tauri 2 + React
│       ├── src-tauri/
│       └── src/
├── docs/
│   └── SPEC.md (이 문서)
└── CLAUDE.md
```

---

## 5. 모듈 상세

### 5.1 Watcher
- 입력: `config.watch[]` (경로, 종류 `local|network`, 폴링 간격, 재귀 여부, 제외 패턴)
- `local`: `notify` 이벤트. `network`: `notify` 시도 + **주기 폴링**(기본 120s, mtime/size 스냅샷 비교) 병행. 둘 다에서 같은 파일이 잡히면 dedupe.
- **안정화 대기**: 생성 이벤트 후 파일 크기가 N초(기본 3s) 동안 변하지 않을 때까지 처리 보류. 임시 파일(`*.tmp`, `*.crdownload`, `*.part`, `~$*`, `.DS_Store`, `Thumbs.db`) 무시.
- 출력: `FileEvent { path, kind: Created|Modified|Removed|Renamed, source: Watch|Poll, at }` → 내부 채널 (M1은 안정화된 `Created|Modified`만 내보낸다. 삭제/이름변경은 추적 대상에서 제거하는 데만 쓴다)
- 자기 자신이 옮긴 파일로 인한 이벤트는 무시(Mover가 발행하는 "expected move" 토큰으로 식별, TTL 10s).
- 다른 프로세스(`tidy undo`)가 되돌린 파일은 토큰이 없으므로, Engine이 `move_log`에서 "같은 원래 경로 + 같은 해시로 undo된 기록"을 확인해 다시 옮기지 않는다. 내용이 바뀌면(해시 다름) 새 파일로 보고 다시 분류한다.
- 감시 루트는 시작 시 정규화(`dunce::canonicalize`)한다. macOS `/var`→`/private/var`, Windows `\\?\` 접두사 문제 회피.
- 잠긴 파일(Windows 공유 위반 등)은 안정화 대기 후 최대 5회 재시도.
- **M6 구현 (watcher/poll.rs)**
  - 폴링 대상: `kind = "network"` 폴더(기본 120초)와 `poll_interval_s`를 직접 준 로컬 폴더. 간격은 최소 5초다. `poll_only = true`인 네트워크 폴더는 OS 알림을 쓰지 않는다.
  - 폴링은 폴더 목록의 (크기, mtime)을 이전 목록과 비교해 새 파일과 바뀐 파일만 안정화 단계로 보낸다(`source = Poll`). 첫 목록은 기준선일 뿐 보고하지 않는다(기존 파일은 `--process-existing`). 메타데이터만 읽는다.
  - dedupe: 안정화 단계가 마지막으로 보고한 (크기, mtime)을 1시간 기억한다. 같은 상태의 폴링 입력은 버린다. 알림으로 이미 처리한 파일이 다음 목록에 나타나도 다시 처리하지 않는다.
  - 연결 끊김: 폴더 목록을 못 읽으면 그 폴더만 offline으로 두고 60초마다 다시 확인한다. 다시 연결되면 OS 알림을 다시 등록하고, 끊기기 전 마지막 목록과 비교해 그동안 생긴 파일을 처리한다. 하위 폴더 하나를 못 읽은 경우에는 그 아래 항목을 이전 목록에서 이어받는다(다음 번에 "새 파일"로 오인하지 않게).
  - 시작할 때 연결이 안 된 네트워크 폴더도 감시 대상으로 둔다(오류 아님). 처음 보이는 순간의 목록을 기준선으로 쓴다.
  - 상태(`RootState`: online / events / poll_secs / files)는 `Watcher::root_states()`로 볼 수 있다. `run_watch`는 바뀔 때마다 `WatchEvent::Root`를 보내고, CLI는 `# offline:` / `# online:`, 앱은 상태줄과 활동 탭에 표시한다.
  - Tidy가 직접 옮겨 놓은 파일은 다시 분류하지 않는다. 엔진은 `move_log`에 "이 경로에 같은 해시로 옮긴 활성 이동"이 있으면 건너뛴다(`placed by Tidy`). 폴링은 수 분 뒤에 보고하므로 expected-move 토큰(10초)으로는 막을 수 없고, 다른 프로세스가 옮긴 파일에도 적용된다. 사용자가 내용을 바꾸면 다시 새 파일로 본다.

### 5.2 Classifier
파이프라인: **Rules → (매칭 없으면) AI → (AI 없거나 confidence 낮으면) Ask user**

**규칙 (rules.toml, 우선순위 순)**
```toml
[[rule]]
name = "스크린샷"
match = { ext = ["png","jpg"], name_regex = "^(스크린샷|Screenshot|화면 캡처)" }
dest = "~/Pictures/Screenshots/{yyyy}/{mm}"
confirm = false

[[rule]]
name = "설치파일"
match = { ext = ["exe","msi","dmg","pkg"] }
dest = "~/Downloads/Installers"
confirm = true
```
- 매치 조건: `ext`, `name_regex`, `min_size`, `max_size`, `older_than_days`, `source_app`(가능한 OS에서만), `from_dir`
  - 조건은 모두 AND. `ext`는 대소문자 무시·점 없이. `name_regex`는 확장자 포함 파일 이름에 적용.
  - 크기는 바이트 정수 또는 `"10MB"`/`"1.5GB"`(1024 단위) 문자열.
  - `source_app`은 SPEC §13 결정 전까지 미지원: 지정한 규칙은 경고 후 매치되지 않음.
  - 규칙 파일의 오타(알 수 없는 키·변수, 잘못된 정규식)는 로드 시 오류로 거부.
- `dest` 템플릿 변수: `{yyyy}` `{mm}` `{dd}` `{ext}` `{stem}` `{source_dir_name}`
  - 날짜는 파일 수정시각(로컬 시간대). `~`는 홈으로 확장. 상대 경로는 원래 파일이 있던 폴더 기준이며, `..`/`.`는 lexical 정규화해 move_log와 출력에 깔끔한 절대경로만 남긴다(`받은 파일/../정리 결과` → `정리 결과`).
- `confirm = false`인 규칙은 묻지 않고 이동(로그는 남김). `confirm = true`면 이동하지 않고 `pending`에 넣는다(M3 확인 UI 전까지 `tidy pending`으로 조회).
- `enabled = false`로 규칙을 끌 수 있다. `ai_auto`는 M4에서 사용.

**AI 분류 요청에 보내는 정보** (프라이버시·비용 위해 최소화)
- 파일명, 확장자, 크기, 생성시각, 원본 폴더
- 본문 앞 N자(기본 1,500자, 설정 가능) — 텍스트 추출 가능할 때만
- 현재 목적지 후보 폴더 트리(깊이 2~3, 최근 사용 폴더 우선)
- 최근 사용자 결정 이력 k건(유사 파일에 대한)

**AI 응답 스키마 (JSON 강제)**
```json
{
  "dest": "~/Documents/업무/KT/과금",
  "confidence": 0.82,
  "reason": "파일명과 본문에 KT 과금 관련 표가 포함",
  "new_folder": false,
  "suggested_rename": null
}
```
- `confidence >= config.ai.auto_threshold`(기본 0.9) **이고** 규칙에 `ai_auto = true`로 허용된 폴더일 때만 자동 이동. 그 외엔 확인 팝업.
- `new_folder = true`면 항상 확인.

**M4 구현 (classify/ai.rs, engine.rs)**
- AI는 **규칙에 맞는 파일이 없을 때만** 부른다. AI가 꺼져 있거나 키가 없으면 M1과 똑같이 동작한다(규칙에 맞지 않는 파일은 그대로 둔다). "Ask user"로 가는 것은 AI가 제안을 냈을 때뿐이다.
- AI 실패(오프라인, 키 오류, 타임아웃, 거절, 스키마 불일치)는 `Action::AiFailed`가 되어 파일을 그대로 둔다. 실패는 `ai_log`에 남기고, 감시 루프나 UI를 막지 않는다. 한 건의 상한은 `ai.timeout_secs`(재시도 포함)다.
- `ai_auto` 폴더: `ai_auto = true` 규칙의 `dest`에서 첫 템플릿 변수 앞까지의 고정 부분이다(예: `~/Documents/업무/{yyyy}` → `~/Documents/업무`). AI 목적지가 그 아래이고, 이미 존재하는 폴더이며, `new_folder = false`일 때만 자동 이동한다. 이 이동은 `move_log.decided_by = ai`, `ai_confidence`로 기록되어 Undo할 수 있다.
- 응답 검증: `dest`는 `~` 확장 후 절대 경로여야 하고 `..`은 정리한다. 존재하지 않는 폴더면 모델이 뭐라고 했든 `new_folder = true`로 본다. `confidence`는 0..1로 자른다. `suggested_rename`은 구분자가 없는 파일 이름만 받는다.
- 후보 폴더 트리: 규칙 `dest`의 고정 부분, `[[index_only]]` 폴더, `ai.candidate_roots`를 `ai.tree_depth`(기본 2)까지 펼친다. 숨김 폴더는 빼고 최대 150줄이다. 최근 이동 목적지 10개, 비슷한 과거 결정 `ai.history_k`(기본 5)건도 함께 보낸다.
- 비용 방지: AI가 대기열에 올린 파일이 다시 저장 이벤트를 내도 AI를 다시 부르지 않는다.
- 사용자가 고른 결과(이동/이름 바꿔 이동/건너뜀/무시)는 규칙 확인과 AI 확인 모두 `decisions`에 남긴다(`source = rule|ai`).

**Learner (학습 루프)**
- 사용자가 확인 팝업에서 내린 결정을 `decisions` 테이블에 저장.
- 같은 `(ext, name 패턴 또는 원본폴더)` 조합으로 같은 `dest`가 3회(설정 가능) 쌓이면 "이 규칙을 자동화할까요?" 제안 → 수락 시 `rules.toml`에 규칙 추가.
- **M4 구현 (classify/learner.rs)**
  - 이름 패턴은 파일 이름에서 첫 숫자나 구분 문자(`_ - . ( [ {`) 앞까지 잘라낸 부분이다. 2글자 이상이어야 한다(`KT과금내역서_2026-09.xlsx` → `KT과금내역서`).
  - 원본 폴더 패턴은 같은 확장자, 같은 목적지에 이름 패턴 제안이 이미 있으면 따로 내지 않는다.
  - 이미 기존 규칙이 묻지 않고 같은 곳으로 보내는 패턴이면 제안하지 않는다.
  - 수락하면 `match = { ext, name_regex = "^접두어" }` 또는 `{ ext, from_dir }`, `confirm = false`인 규칙을 `rules.toml`의 **첫 규칙 앞**에 주석과 함께 넣는다. 더 구체적인 규칙이 먼저 매치되도록 하기 위해서다. 파일은 검증을 통과해야만 쓴다.
  - "다시 묻지 않기"는 `learn_dismissed`에 남긴다.
  - CLI는 `tidy learn [--accept N | --dismiss N]`이고, 앱에서는 대기열 탭 위의 제안 카드로 처리한다.

### 5.3 Confirm UI (사용자 확인)
- 트레이 알림 → 클릭 시 작은 팝업: 파일명, 미리보기(텍스트 앞부분/이미지 썸네일), 제안 목적지, 이유, 대안 폴더 3개
- 액션: **이동 / 다른 곳 선택 / 이름 바꿔 이동 / 건너뜀(이번만) / 무시(이 파일 영구)**
- 미응답 파일은 **대기열(Pending)** 에 쌓이고 검색창에서 일괄 처리 가능.
- 조용 모드(설정): 알림 없이 Pending에만 쌓음.
- **M3 구현 범위**
  - 트레이 알림을 클릭하는 단계 없이, 확인이 필요한 파일이 생기면 팝업 창을 화면 오른쪽 아래에 **포커스를 뺏지 않고** 바로 띄운다(OS 알림 클릭 → 창 열기가 플랫폼마다 불안정해서). 조용 모드면 띄우지 않는다.
  - 액션은 이동 / 다른 곳…(폴더 선택) / 건너뜀 / 무시. "이름 바꿔 이동", 미리보기(썸네일), 대안 폴더 3개는 AI 제안이 생기는 M4에서 추가한다.
  - 처리 로직은 코어 `pending::resolve` 하나이며 CLI(`tidy pending --move/--skip/--ignore <ID> [--to DIR]`)와 같다. 이동은 `decided_by = user`로 `move_log`에 남고 Undo 가능.
- **M4 추가**: AI 제안이면 "AI 82%" 배지와 이유, 새 이름 제안을 보여 준다. "이름 바꿔 이동"(CLI `--rename NAME`)은 새 이름에 확장자가 없으면 원래 확장자를 붙이며, `decisions.action = rename_move`로 남긴다. 미리보기 썸네일과 대안 폴더 3개는 아직 없다(응답 스키마에 대안이 없음 — 필요해지면 스키마부터 바꾼다).
  - 건너뜀 = 대기열에서만 뺀다(파일이 다시 바뀌면 재분류). 무시 = `ignored` 테이블에 넣어 그 경로는 다시 정리하지 않는다.

### 5.4 Mover + Undo
- 이동 절차:
  1. 목적지 충돌 검사 → 동일 해시면 중복 처리 정책(건너뜀/덮어쓰기/이름 뒤 `(1)`), 다른 파일이면 이름 변경
  2. 같은 볼륨: `rename`. 다른 볼륨/NAS: `dest.tmp.<uuid>`로 복사 → 크기·해시 검증 → 원자적 rename → 원본 삭제
  3. `move_log`에 기록 (before/after 경로, 해시, 결정 출처 `rule|ai|user|restructure`, 배치 ID)
  - 구현 순서: 로그 행을 **먼저** 쓰고 이동한다. 이동이 실패하면 그 행을 철회한다. 따라서 기록 없는 이동은 생길 수 없다.
  - 이름이 같고 내용이 다른 파일은 항상 `이름 (1).ext`로. 어떤 경우에도 덮어쓰지 않는다. `replace`는 동일 해시 사본을 휴지통으로 보낸 뒤 이동.
  - 경로는 UTF-8 TEXT로 저장한다. UTF-8이 아닌 경로는 손실 없이 기록할 수 없으므로 이동하지 않는다.
  - 복사-검증 이동의 임시 파일 이름: `.<name>.tidy-tmp.<uuid>` (Watcher가 무시).
  - **M6 복사-검증 (mover/transfer.rs)**
    - `rename`이 다른 볼륨 오류(EXDEV / Windows `ERROR_NOT_SAME_DEVICE`)를 내면 복사-검증으로 넘어간다.
    - 복사하면서 원본을 해시하고 fsync한다. 그 해시가 `move_log`에 기록한 해시와 다르면 원본이 그사이 바뀐 것이므로 옮기지 않는다.
    - 사본을 다시 읽어 크기와 blake3를 비교하고, 원래 mtime을 붙인 뒤 rename으로 제자리에 둔다. 그다음에 원본을 지운다.
    - 원본을 지울 수 없으면(권한, 연결 끊김) 방금 만든 사본을 지우고 오류로 끝낸다. 사본은 원본과 같은 내용이므로 데이터 손실이 없고, 파일이 기록 없이 두 곳에 남지 않는다.
    - 실패하면 임시 파일만 지운다. 원본은 건드리지 않는다.
    - 긴 복사는 250ms마다 진행률(`TransferProgress { copied, total, phase: copy|verify|done }`)을 보고한다. 250ms 안에 끝나는 복사는 보고하지 않는다.
    - 복사-검증 이동과 네트워크 폴더 본문 읽기는 프로세스 전체에서 `[mover] nas_concurrency`(기본 2)개까지만 동시에 한다(`netio`).
  - **M6 상태와 중단 복구 (mover/recover.rs)**
    - `move_log.state`: 로그 행을 `moving`으로 쓰고, 이동하고, `done`으로 바꾼다. 실패하면 행을 철회한다.
    - `moving`으로 남은 행은 프로세스가 중간에 멈춘 것이다. `watch` 시작 시, 네트워크 폴더가 다시 연결될 때, `tidy recover`로 정리한다.
      - 원본 없음 + 목적지 있음 → 끝난 이동이다. `done`으로 바꾼다.
      - 원본 있음 + 목적지 없음 → 안 일어난 이동이다. 행을 철회하고 남은 임시 파일을 지운다. 목적지 폴더에 접근할 수 없으면(NAS offline) 다음 번으로 미룬다.
      - 둘 다 있고 둘 다 기록된 해시와 같음 → 원본 정리 직전에 멈춘 것이다. 원본을 지우고 `done`으로 바꾼다.
      - 둘 다 있는데 내용이 다름 → 아무것도 지우지 않는다. 행을 철회하고 "확인 필요"로 보고한다.
      - 둘 다 없음 → 공유가 offline인 것으로 보고 다음 번으로 미룬다.
    - 15분(`DEFAULT_RECOVERY_GRACE`)이 안 지난 행, 이 프로세스에서 진행 중인 이동, 그 사이에 쓰인 임시 파일이 있는 행은 건드리지 않는다. 다른 Tidy 프로세스가 긴 복사를 하고 있을 수 있기 때문이다. 임시 파일의 mtime은 검증이 끝난 뒤에야 원본 값으로 바꾸므로, 복사 중인 사본은 항상 새것으로 보인다. `tidy recover --grace-secs 0`은 다른 Tidy가 없을 때만 쓴다.
    - 끝나지 않은 행은 Undo하지 않는다(충돌로 보고).
- **Undo**: 단건/배치 단위 되돌리기. 원본 위치에 다른 파일이 생겼으면 충돌 알림. 최근 30일(설정) 보관.
  - CLI: `tidy undo --last`(가장 최근의 미취소 배치), `--batch <id 접두사>`, `--id <move_log id>`.
  - 충돌(원래 위치에 다른 파일 존재 / 옮긴 파일이 사라짐)은 건너뛰고 보고하며, 해당 행은 활성 상태로 남아 재시도 가능.
  - 원래 폴더가 없어졌으면 다시 만든다. 이동 때 생긴 빈 목적지 폴더는 지우지 않는다(삭제 금지).
  - `undo_retention_days`가 지난 로그 행은 `watch` 시작 시 DB에서만 정리된다(파일은 건드리지 않음).
  - 배치: watch 중 파일 1건 = 배치 1개, `--process-existing` 초기 처리 = 배치 1개.
- 절대 삭제하지 않는다. "삭제"는 OS 휴지통으로 보내는 것만 허용(`trash` crate).

### 5.5 Indexer
- 대상: 감시 폴더 전체 + 사용자가 추가 지정한 인덱스 전용 폴더
- 초기 전체 스캔 → 이후 Watcher 이벤트로 증분 갱신
  - M3: 감시 루프(`service::run_watch`)는 처리한 파일마다 인덱스를 갱신한다. 이동된 파일은 원래 경로의 행을 지우고 `Indexer::index_file(새 경로)`, 대기열·규칙 없음 파일은 제자리에서 인덱싱한다. Undo와 확인 팝업 이동도 같은 방식으로 반영한다. 인덱스 실패는 경고만 남기고 이동을 막지 않는다.
  - `index_file`은 어떤 루트에도 속하지 않는 파일(예: 규칙이 `~/Pictures/...`로 옮긴 파일, `tidy index <파일>`)이면 그 파일의 폴더를 `watch = 0` 루트로 등록한다. 그 폴더 전체를 훑지는 않는다.
  - 앱은 시작할 때와 설정을 저장할 때 감시 폴더·`index_only` 폴더를 백그라운드에서 증분 인덱싱한다(작은 트랜잭션 50건 단위로 써서 감시 루프의 DB 쓰기를 오래 막지 않는다).
- 각 파일: `path, size, mtime, ctime, ext, mime, hash(blake3), text_excerpt, full_text(FTS), embedding(optional)`
- 증분: 크기+mtime이 같으면 `last_seen_at`만 갱신하고 건너뛴다. 바뀐 경우에만 해시·본문 재처리. 대용량(기본 >200MB, `index.max_content_size_mb`)은 해시·본문 스킵하고 메타만(`text_status = skipped`). 실행이 끝나면 그 루트에서 이번 실행에 보이지 않은 행을 제거한다(파일은 건드리지 않음).
- 텍스트 추출기는 `trait Extractor { fn supports(ext) -> bool; fn extract(path) -> Result<String> }`로 확장. 실패해도 메타 인덱싱은 유지(`text_status = failed`). 추출기 패닉은 잡아서 실패로 처리.
  - M2 추출기: 텍스트/소스코드(UTF-8·UTF-16 BOM·CP949 자동 판별), HTML(태그 제거), PDF(pdf-extract), DOCX/PPTX(zip+XML), XLSX/XLS/ODS(calamine). 본문은 `index.max_text_chars`(기본 1,000,000자)에서 자른다.
- CLI: `tidy index [<폴더>...] [--no-content] [--rebuild]` — 인자가 없으면 설정의 `[[watch]]`·`[[index_only]]` 루트 전부. `tidy index --status`로 인덱스 통계.
- NAS 폴더: 기본 **메타만** 인덱싱. 본문 인덱싱은 설정으로 켜되 야간 시간대/수동 트리거 옵션 제공.
  - **M6 구현**: `kind = "network"`는 `index_content`를 생략하면 false다.
    - 해시는 `index.max_hash_size_mb_network`(20MB) 이하 파일만 만든다.
    - 백그라운드 인덱싱(앱 시작, 설정 저장, NAS 시간대 폴더가 있으면 1시간마다)은 `content_index_window`("HH:MM-HH:MM", 자정을 넘겨도 됨) 안에서만 본문을 읽는다. `tidy index`(수동)는 시간대를 무시한다.
    - 메타만 인덱싱된 파일(`text_status = none`)은 크기와 mtime이 같아도, 본문을 읽을 때가 되면 다시 처리한다.
    - 감시·이동·Undo 뒤 단일 파일을 인덱싱할 때도, 파일이 네트워크 폴더 안에 있으면 그 폴더 설정을 따른다(`IndexOptions::for_file`).
    - 네트워크 폴더의 본문 읽기는 NAS IO 슬롯을 하나 쓴다.
- 우선순위: 백그라운드 저우선 스레드, 인덱싱 중 CPU/IO 상한(설정) — 사용자 작업 방해 금지.

### 5.6 Search
- 검색창(글로벌 단축키, 기본 `Ctrl/Cmd+Shift+Space`)
- 문법: 일반 키워드 → FTS5(파일명 가중치 3 > 본문 1, bm25). 키워드는 AND, `"따옴표"`는 구절. 필터 `ext:pdf,docx` `in:~/Documents`(공백 있으면 `in:"..."`) `after:2026-09` `before:2026-10-15` `size:>10mb` `type:image|doc|video|audio|archive|code`
  - 토크나이저 라우팅(docs/fts-tokenizer.md): 3글자 이상 항목은 trigram 테이블, 1~2글자 항목은 unicode61 테이블 prefix 매치. 둘이 섞이면 교집합.
  - CLI: `tidy search "질의" [-n 20] [--format json]`
- 의미검색: 임베딩이 켜져 있으면 FTS 결과와 벡터 결과를 RRF로 병합. 자연어 질문형 입력이면 의미검색 가중치 상향.
- 결과: 경로, 매칭 스니펫(하이라이트), 수정일. 액션: 열기 / 폴더 열기 / 이동 / 복사 경로
- 응답 목표: 로컬 FTS 50ms 이내, 벡터 포함 300ms 이내 (10만 파일 기준)

### 5.7 Reporter
- 트리 출력(깊이 제한, 용량/파일 수 표시, 텍스트/JSON/HTML 내보내기)
- 통계: 타입별 용량, 폴더별 용량 Top N, 중복 파일 그룹(해시 기준), 90일 이상 미접근 파일, 빈 폴더
- **AI 재구성 제안**: 현재 트리 요약(깊이 3, 폴더별 파일 수/대표 파일명)을 보내 "제안 트리 + 이동 목록" JSON을 받음 → 사용자에게 **before/after diff** 보여줌 → 승인 시 Mover 배치 실행(배치 Undo 가능)
- 재구성 제안은 **절대 자동 적용하지 않는다.**
- **M5 구현 (report/)**
  - `tidy report [폴더] [--format text|json|html]`(HTML은 stdout으로 내보냄 — 코어는 파일을 쓰지 않는다)와 앱 리포트 탭을 쓴다.
  - 중복 파일: 같은 크기끼리만 blake3로 비교한다. 1KB 미만과 2GB 초과는 제외한다. 크기가 같으면 인덱스에 저장된 해시를 재사용한다. 보고만 하고 삭제 기능은 없다(필요하면 사용자가 폴더를 열어 직접 휴지통으로 보낸다).
  - 미접근 파일: 수정 시각과 접근 시각 중 늦은 쪽이 `report.stale_days`(90)보다 오래된 파일이다. Windows는 접근 시각 갱신이 꺼져 있는 경우가 많아 사실상 수정 시각 기준이다.
  - **재구성 계획은 두 출처로 만든다.**
    1. **규칙 계획**(`--rules`): AI 없이, 지금 있는 파일에 rules.toml을 적용했을 때의 이동 목록이다(CLAUDE.md 규칙 4). `confirm = true` 규칙도 포함하는데, 계획 적용 자체가 확인이기 때문이다.
    2. **AI 계획**(`--ai`): §8.2 프롬프트를 쓴다. 트리 요약(깊이 3 폴더별 파일 수·용량·예시 5개, 루트 파일은 최대 300개)을 보내고 `{summary, folders[{path, reason}], moves[{from, to, reason}]}`를 받는다. `max_tokens`는 16000, 시간 상한은 `ai.restructure_timeout_secs`(180s)다.
  - AI 이동 검증(통과 못 한 항목은 `rejected`에 이유와 함께 보여 주고 절대 적용하지 않음):
    - 루트 기준 상대 경로만 받는다. 절대 경로, 드라이브, UNC, `..`은 거부한다.
    - 원본이 실제로 있어야 하고, "건드리지 말 폴더"(`[report] protected`와 요청별 목록)의 안팎을 건드리면 안 된다.
    - 폴더를 자기 안으로 옮기는 것, 다른 이동과 겹치는 것, 같은 목적지로 가는 중복 이동, 빈 폴더 이동, 상한(`report.max_moves` 500) 초과도 거부한다.
    - 폴더 이동은 안의 파일 전체를 구조 그대로 옮긴다.
  - 계획은 `restructure_plans`에 저장한다. 미리보기는 선택한 항목 기준 before/after 트리(깊이 3)이고, 루트 밖으로 나가는 파일과 **비게 되는 폴더**(남겨 둠)를 함께 보여 준다.
  - 적용(`--apply ID [--only 1,3]` / 앱 "선택한 N개 적용" + 확인 대화상자):
    - 선택한 항목만 파일 단위로 펼쳐 Mover로 옮긴다. 전부 **한 배치**(`decided_by = restructure`, `rule_name = "재구성 #ID"`)이고 인덱스도 따라간다.
    - 같은 계획은 다시 적용할 수 없다(그 배치를 되돌리면 된다).
    - 사이에 사라진 파일과 동일한 내용이 이미 있는 파일은 건너뛰고 보고한다. 비게 된 폴더는 지우지 않는다.

### 5.8 AI Layer
```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete_json(&self, system: &str, user: &str, schema: &str) -> Result<serde_json::Value>;
    fn name(&self) -> &str;
}
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
}
```
- 구현체: `AnthropicProvider`, `OpenAiProvider`(OpenAI 호환 엔드포인트 — 다른 벤더도 커버), `OllamaProvider`(로컬)
- **M4 구현**
  - 실제 트레이트는 `complete_json(system, user, schema: &serde_json::Value) -> Result<LlmResponse { value, usage, model }, AiError>`이다. 사용량과 응답 모델을 `ai_log`에 남기려고 위 스케치보다 정보를 더 돌려준다.
  - `EmbeddingProvider`는 선언만 해 두었고, 구현은 M7에서 한다.
  - Anthropic은 공식 Rust SDK가 없어 raw HTTP로 부른다. 요청은 `POST /v1/messages`, 헤더는 `x-api-key`와 `anthropic-version: 2023-06-01`이다. 답은 `output_config.format = {type: json_schema}`로 JSON만 받는다. 프로필의 `effort`(예: `"low"`)는 `output_config.effort`로 보낸다. `stop_reason = refusal`은 실패로 처리한다.
  - OpenAI 호환은 `/chat/completions`에 `response_format = json_schema(strict)`로 보낸다. 400/422가 오면 한 번만 `json_object` 모드로 다시 보낸다(스키마는 프롬프트에 넣는다).
  - Ollama는 `/api/chat`에 `format = 스키마`, `stream = false`, `temperature = 0`으로 보낸다.
  - HTTP는 `reqwest`(rustls/ring, OpenSSL·cmake 불필요)를 쓴다. 408/409/429/5xx(529 포함)와 네트워크 오류는 지수 백오프로 재시도하고(`retry-after` 존중, 최대 8s), 횟수는 `ai.max_retries`(기본 2)다. 400 같은 클라이언트 오류는 재시도하지 않는다. 별도 레이트 리미터는 아직 없다(429 재시도로 대신한다).
  - 엔진은 동기 코드라서 `AiClassifier`가 자체 current-thread tokio 런타임으로 `block_on`한다. 감시 스레드나 `spawn_blocking` 스레드에서만 부르고, async 컨텍스트 안에서는 부르지 않는다.
  - 키 저장: `keyring` 3을 쓴다. Windows는 자격 증명 관리자, macOS는 키체인, Linux는 커널 keyutils다. `key_ref = "service/user"`이고 `/`가 없으면 service는 `tidy`다. 키는 로그·설정·UI 응답 어디에도 나가지 않는다(`Debug` 출력도 가린다). 앱의 키 입력칸은 쓰기 전용이고, 화면에는 "저장됨" 여부만 보인다.
  - 비용 추정: 알려진 Claude 모델 단가표(`ai::estimate_cost_usd`)로 계산한다. Ollama는 0이고, 모르는 모델은 NULL이다.
  - `ai_log`에는 제공자, 모델, 목적(classify/test), 토큰, 비용, 지연, 성공 여부, 오류, 결과 요약(목적지와 확신도)만 남긴다. 파일 본문과 프롬프트는 남기지 않는다.
  - 자동화된 테스트는 `MockProvider`와 로컬 가짜 HTTP 서버만 쓴다. 실제 API는 수동 테스트(`tidy ai test <파일>`, 앱 "연결 테스트")로만 확인한다.
- 설정에서 **분류용 모델**과 **임베딩 모델**을 따로 지정. 예: 분류는 클라우드, 임베딩은 로컬.
- 프로필: `[ai.profiles.work]`, `[ai.profiles.home]` — 회사 PC에서는 로컬 전용 프로필 강제 가능. `send_content = false`면 본문을 AI에 보내지 않고 메타만 보냄.
- 키는 OS 키체인(`keyring` crate)에 저장. config.toml에는 키 이름만.
- 레이트 리밋·재시도·타임아웃(기본 20s). 실패 시 규칙/Ask user로 폴백, 절대 블로킹하지 않음.
- 모든 AI 요청/응답 요약은 `ai_log`에 남김(토큰 수, 비용 추정, 결과).

### 5.9 NAS 처리 정책 (1차)
- 네트워크 경로는 사용자가 `kind = "network"`로 명시(자동 감지는 보조: UNC 경로, 마운트 타입 확인).
- 감시: 이벤트 + 폴링. 연결 끊김 시 해당 경로만 일시정지하고 재연결 폴링(60s).
- 이동: 복사-검증-삭제 방식. 진행률 표시. 중단 시 `.tmp` 정리.
- 인덱싱: 메타 기본, 본문 옵트인. 해시는 작은 파일(기본 <20MB)만.
- 성능 상한: NAS 동시 IO 1~2개로 제한.
- 2차에서 NAS 측 에이전트로 대체 가능하도록, Watcher/Indexer는 "원격 소스"를 플러그인 형태로 받을 수 있게 인터페이스를 열어둔다.
- **M6 구현 요약** (세부는 §5.1, §5.4, §5.5)
  - 자동 감지(`paths::looks_like_network`)는 보조로만 쓴다. 대상은 UNC, Windows 원격 드라이브(`GetDriveTypeW`), Linux `mountinfo`·macOS `statfs`의 SMB/NFS/AFP/WebDAV/SSHFS다. 쓰임은 두 가지다.
    - 앱 "폴더 추가"의 기본 종류를 정한다.
    - `local`로 둔 네트워크 폴더에 대해 시작 시 경고한다.
  - 시작 경고(`Config::warnings`): DB가 네트워크 폴더 아래이거나 네트워크 위치로 보일 때, 그리고 `content_index_window` 형식 오류. 오프라인 공유에서 막히지 않도록 경로를 해석하지 않고 문자열로만 비교한다.
  - "원격 소스" 플러그인 인터페이스는 아직 만들지 않았다. 폴링은 `Watcher` 안의 별도 스레드이고, 2차 에이전트로 바꿀 때 이 경계를 쓴다.
  - 완료 기준 테스트(`tests/m6_flow.rs`)는 임시 폴더를 NAS로 쓰고, 이름을 바꿔 "마운트 해제/재마운트"를 흉내 낸다.
    - 폴링으로 이동되는지 확인한다.
    - 끊김 → 그 폴더만 offline이 되는지 확인한다.
    - 끊긴 동안 추가된 파일이 재연결 후 처리되는지 확인한다.
    - 두 이동 모두 Undo되는지 확인한다.
    - 복사-검증 이동, 진행률, 메타 인덱싱, 재분류 방지도 함께 확인한다.

---

## 6. 데이터 모델 (SQLite)

```sql
-- 인덱싱된 파일
CREATE TABLE files (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  root_id INTEGER NOT NULL REFERENCES roots(id),
  name TEXT NOT NULL,
  ext TEXT,
  mime TEXT,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  ctime INTEGER,
  hash TEXT,                  -- blake3 hex, NULL if skipped
  text_status TEXT NOT NULL DEFAULT 'none',  -- none|ok|unsupported|failed|skipped
  excerpt TEXT,               -- 앞 500자
  content TEXT,               -- 추출 본문 전체(index.max_text_chars까지), FTS external content
  indexed_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL  -- 마지막 인덱싱 실행에서 본 시각. 안 보인 행은 실행 후 제거
);
CREATE INDEX idx_files_hash ON files(hash);
CREATE INDEX idx_files_ext ON files(ext);
CREATE INDEX idx_files_mtime ON files(mtime);

-- 전문검색 (M2 결정, docs/fts-tokenizer.md)
-- files.content(추출 본문)를 external content로 공유한다: 본문은 한 번만 저장되고
-- snippet()/highlight()가 동작한다. 동기화는 files의 INSERT/DELETE/UPDATE OF name,content 트리거.
CREATE VIRTUAL TABLE files_fts USING fts5(            -- 주: 3글자 이상 질의
  name, content, content='files', content_rowid='id', tokenize='trigram'
);
CREATE VIRTUAL TABLE files_fts_word USING fts5(       -- 보조: 1~2글자 질의를 prefix(`*`)로
  name, content, content='files', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);

-- 임베딩 (sqlite-vec 사용 시 vec0 테이블, 아니면 BLOB)
CREATE TABLE embeddings (
  file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  dim INTEGER NOT NULL,
  vec BLOB NOT NULL,
  created_at INTEGER NOT NULL
);

-- 감시/인덱스 루트
CREATE TABLE roots (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,          -- local|network
  watch INTEGER NOT NULL,      -- 0/1
  index_content INTEGER NOT NULL,
  poll_interval_s INTEGER
);

-- 이동 로그 (Undo 근거)
CREATE TABLE move_log (
  id INTEGER PRIMARY KEY,
  batch_id TEXT NOT NULL,
  src_path TEXT NOT NULL,
  dst_path TEXT NOT NULL,
  hash TEXT,
  size INTEGER,
  decided_by TEXT NOT NULL,    -- rule|ai|user|restructure
  rule_name TEXT,
  ai_confidence REAL,
  moved_at INTEGER NOT NULL,
  undone_at INTEGER,
  state TEXT NOT NULL DEFAULT 'done'   -- (M6) moving|done: 중단 복구용
);
CREATE INDEX idx_move_batch ON move_log(batch_id);
CREATE INDEX idx_move_dst ON move_log(dst_path);   -- (M6) Tidy가 옮겨 놓은 파일 재분류 방지
CREATE INDEX idx_move_src ON move_log(src_path);   -- undo된 파일 재이동 방지 조회

-- 사용자 결정 (학습용)
CREATE TABLE decisions (
  id INTEGER PRIMARY KEY,
  file_name TEXT NOT NULL,
  ext TEXT,
  src_dir TEXT NOT NULL,
  suggested_dest TEXT,
  chosen_dest TEXT,            -- NULL = 건너뜀
  action TEXT NOT NULL,        -- move|skip|ignore|rename_move
  source TEXT NOT NULL DEFAULT 'rule',  -- (M4) rule|ai: 누가 대기열에 올렸나
  decided_at INTEGER NOT NULL
);
CREATE INDEX idx_decisions_ext ON decisions(ext);

-- (M4) 학습 제안 "다시 묻지 않기"
CREATE TABLE learn_dismissed (key TEXT PRIMARY KEY, dismissed_at INTEGER NOT NULL);

-- 대기열
CREATE TABLE pending (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  suggested_dest TEXT,
  reason TEXT,
  confidence REAL,
  alternatives TEXT,           -- JSON array
  created_at INTEGER NOT NULL,
  source TEXT NOT NULL DEFAULT 'rule',  -- (M4) rule: reason=규칙 이름 / ai: reason=AI 이유
  suggested_name TEXT                   -- (M4) AI 새 이름 제안
);

-- 영구 무시
CREATE TABLE ignored (
  path TEXT PRIMARY KEY,
  added_at INTEGER NOT NULL
);

-- AI 호출 로그
CREATE TABLE ai_log (
  id INTEGER PRIMARY KEY,
  provider TEXT, model TEXT, purpose TEXT,   -- classify|embed|restructure
  input_tokens INTEGER, output_tokens INTEGER,
  cost_usd REAL, latency_ms INTEGER,          -- (M4)
  ok INTEGER NOT NULL, error TEXT,
  result TEXT,                                -- (M4) "목적지 (82%)" 요약, 본문 없음
  called_at INTEGER NOT NULL
);

-- (M5) 재구성 계획: 제안 → 사용자 승인 → 한 배치로 적용
CREATE TABLE restructure_plans (
  id INTEGER PRIMARY KEY,
  root TEXT NOT NULL,
  source TEXT NOT NULL,        -- rules|ai
  created_at INTEGER NOT NULL,
  plan_json TEXT NOT NULL,     -- 이동 목록·이유·거부 항목
  applied_batch TEXT,          -- move_log.batch_id
  applied_at INTEGER
);

CREATE TABLE schema_version (version INTEGER NOT NULL);
```

마이그레이션: `schema_version`에 적용 버전을 기록하고 순서대로 적용. M1(v1)은 `move_log`, `pending`, `ignored`, M2(v2)는 `roots`, `files`, `files_fts`, `files_fts_word`를 만든다. M4(v3)는 `pending.source/suggested_name` 열과 `decisions`, `ai_log`, `learn_dismissed`를 추가한다. M5(v4)는 `restructure_plans`를 추가한다. M6(v5)는 `move_log.state`와 `dst_path`·`state` 인덱스를 추가한다(기존 행은 `done`). 나머지 테이블은 해당 마일스톤에서 추가.

**DB 파일 위치**: SQLite는 파일 잠금이 필요하므로 `store.path`는 반드시 로컬 디스크여야 한다. NAS/공유 폴더/네트워크 드라이브에 두면 `disk I/O error`로 실패한다(에러 메시지에 힌트를 붙인다. M6: `kind = "network"` 루트 아래이거나 네트워크 위치로 보이는 store.path는 기동 시 경고 — CLI `# warning:`, 앱 활동 탭 "주의").

---

## 7. 설정 파일 (`config.toml`)

```toml
[general]
language = "ko"
hotkey = "CmdOrCtrl+Shift+Space"
quiet_mode = false
start_on_login = true

[[watch]]
path = "~/Desktop"
kind = "local"
recursive = false
index_content = true

[[watch]]
path = "~/Downloads"
kind = "local"
recursive = false
index_content = true

[[watch]]
path = "Z:/공유/업무"           # 또는 "/Volumes/nas/업무"
kind = "network"
recursive = true
poll_interval_s = 120          # 최소 5
index_content = false          # NAS는 메타만 (network는 생략 시 false)
content_index_window = "02:00-05:00"
poll_only = false              # (M6) true = OS 알림 없이 폴링만

[[index_only]]
path = "~/Documents"
recursive = true

[classify]
rules = "rules.toml"           # 상대 경로는 config.toml 폴더 기준
stabilize_secs = 3
learn_threshold = 3
ignore_globs = ["*.tmp", "*.crdownload", "*.part", "~$*", ".DS_Store", "Thumbs.db"]

[mover]
undo_retention_days = 30
duplicate_policy = "skip"      # skip|rename|replace
nas_concurrency = 2

[store]
# path = "tidy.db"             # 생략 시 <data_local_dir>/tidy/tidy.db, 상대 경로는 config.toml 폴더 기준
                               # 반드시 로컬 디스크. 네트워크 경로면 SQLite disk I/O error

[index]
max_content_size_mb = 200      # 초과 시 메타만
max_text_chars = 1000000       # 파일당 저장 본문 상한(글자)
max_hash_size_mb_network = 20
cpu_limit_percent = 30

[ai]
enabled = true
active_profile = "home"
auto_threshold = 0.9
send_content = true
content_chars = 1500
timeout_secs = 20              # (M4) 한 건 상한(재시도 포함)
max_retries = 2                # (M4) 429/5xx/네트워크 오류 재시도
tree_depth = 2                 # (M4) 후보 폴더 트리 깊이
history_k = 5                  # (M4) 예시로 보내는 과거 결정 수
candidate_roots = []           # (M4) 후보 트리에 추가할 폴더
restructure_timeout_secs = 180 # (M5) AI 재구성 제안 시간 상한

[report]                       # (M5)
protected = []                 # 재구성이 절대 건드리지 않는 폴더
stale_days = 90
top_n = 10
max_moves = 500                # AI 재구성 한 번에 받는 이동 상한

[ai.profiles.home]
classify = { provider = "anthropic", model = "claude-haiku-5-5", key_ref = "tidy/anthropic", effort = "low" }
embed    = { provider = "ollama", model = "nomic-embed-text", base_url = "http://localhost:11434" }

[ai.profiles.work]
classify = { provider = "ollama", model = "qwen3:8b", base_url = "http://localhost:11434" }
embed    = { provider = "ollama", model = "nomic-embed-text", base_url = "http://localhost:11434" }
send_content = false
```

---

## 8. AI 프롬프트 설계

### 8.1 분류 (system)
```
너는 파일 정리 보조자다. 주어진 파일 정보와 사용자의 폴더 구조, 과거 결정을 보고
이 파일이 들어가야 할 가장 적절한 폴더를 하나 고른다.
- 기존 폴더를 우선한다. 새 폴더는 정말 필요할 때만 제안하고 new_folder=true로 표시한다.
- 사용자 과거 결정과 일관되게 판단한다.
- 확신이 없으면 confidence를 낮게 준다. 거짓 확신 금지.
- 반드시 아래 JSON 스키마로만 응답한다.
```
### 8.2 재구성 (system)
```
너는 폴더 구조 컨설턴트다. 주어진 트리 요약을 보고 더 찾기 쉬운 구조를 제안한다.
- 폴더 깊이는 최대 3을 권장한다.
- 이동 목록은 {from, to} 배열로, 삭제는 절대 제안하지 않는다.
- 변경 이유를 폴더 단위로 짧게 설명한다.
- 사용자가 명시한 "건드리지 말 폴더"는 제외한다.
```
프롬프트 본문은 `ai/prompts.rs`에 상수로 두고 테스트 케이스와 함께 관리한다.

---

## 9. UI 화면 목록 (Tauri/React)

1. **트레이 메뉴**: 검색 열기 / 대기 N건 / 일시정지 / 리포트 / 설정 / 종료
2. **검색창**: 입력 + 결과 리스트 + 우측 미리보기. 키보드 전용 조작 가능.
3. **확인 팝업**: 5.3 참조. 작은 창, 항상 위.
4. **대기열**: 테이블, 다중 선택 후 일괄 이동/건너뜀.
5. **리포트**: 트리 뷰, 통계 카드, 중복 그룹, 재구성 제안 diff(before/after 트리 + 이동 목록 체크박스).
6. **이동 기록 / Undo**: 배치 단위 타임라인, 되돌리기 버튼.
7. **설정**: 감시 폴더, 규칙 편집(폼 + raw TOML), AI 프로필/키, 단축키.

### 9.1 M3 구현 (apps/desktop)
- 구성: Tauri 2 + React 19 + TypeScript + zustand. `src-tauri`는 `tidy-core`를 직접 호출하는 얇은 어댑터이고, 루트 Cargo 워크스페이스에서 `exclude`한 독립 crate다(GUI 라이브러리가 없는 환경에서도 `cargo test --workspace`가 돌도록).
- 창 3개(번들 하나, URL 해시로 화면 선택): `main`(활동 / 대기열 / 설정 탭), `search`(테두리 없음, 항상 위, 포커스를 잃으면 숨김), `confirm`(작은 창, 항상 위). 닫기 버튼은 숨기기만 하고 앱은 트레이에 남는다. 종료는 트레이 "종료".
- 트레이 메뉴: 검색 열기 / 대기 N건 / 일시정지·감시 재개 / 지금 인덱싱 / 설정 / 종료. 왼쪽 클릭은 메인 창(활동 탭)을 연다. 리포트 메뉴는 M5에서 추가.
- 글로벌 단축키 `general.hotkey`(기본 `CmdOrCtrl+Shift+Space`)로 검색창을 토글한다. 다른 앱과 겹쳐 등록에 실패하면 상태줄에 오류를 띄우고 앱은 계속 동작한다.
- 검색창: FTS 결과 + 이름/본문 하이라이트 + 스니펫. ↑↓ 선택, Enter 열기, Ctrl+Enter 폴더 열기, Esc 닫기, 경로 복사. 하이라이트는 백엔드가 사설 영역 문자(U+E000/U+E001)로 표시하고, 프론트는 `<mark>`로 렌더링한다(innerHTML 미사용).
- 설정: 감시 폴더(추가·삭제, 하위 폴더, 본문 인덱싱, 종류), 규칙 raw TOML(입력할 때마다 검증하고 오류가 있으면 저장 불가), DB 경로, 단축키, 조용 모드. `config.toml`은 `toml_edit`로 해당 키만 바꾸므로 주석과 다른 키는 그대로 남는다. 규칙 파일은 검증을 통과해야만 덮어쓴다.
- 설정 파일 위치: `--config <경로>` → 환경변수 `TIDY_CONFIG` → OS 기본(`%APPDATA%\tidy\config.toml`). 파일이 없으면 감시 폴더 없이 시작하고 설정 탭을 연다. 사용자가 저장하기 전에는 아무것도 옮기지 않는다.
- 동시성: 감시 루프는 자체 스레드와 DB 연결을 쓴다. UI 명령은 두 번째 연결을 쓰고, "expected move" 토큰을 감시 루프와 공유해 팝업에서 옮긴 파일을 새 파일로 오인하지 않는다. 모든 명령은 async + `spawn_blocking`이다. 감시 재시작(일시정지·설정 저장)은 보조 스레드에서 직렬화한다. 메인 스레드가 감시 스레드를 join하면, 감시 스레드가 부르는 트레이·창 API와 교착될 수 있기 때문이다.
- 이동 기록 화면(§9.6)은 M5 범위다. M3에는 활동 탭의 실시간 로그와 "마지막 이동 되돌리기" 버튼만 있다.
- **M5 추가**
  - 트레이 메뉴에 "리포트"를 추가했다.
  - **리포트 탭**
    - 폴더를 고른다(감시·index_only 폴더 또는 다른 폴더…).
    - 통계 카드, 종류별, 큰 폴더/파일, 중복 그룹, 오래 안 쓴 파일, 빈 폴더, 트리를 보여 준다. 경로를 누르면 탐색기에서 선택된다.
    - 정리 제안: "규칙으로 정리 미리보기" / "AI 재구성 제안" / 지난 제안을 연다. 이동 체크박스를 바꾸면 before/after 트리가 바로 갱신된다. "선택한 N개 적용"은 확인 대화상자를 거치고, 적용 뒤에는 "이 재구성 되돌리기"가 있다.
  - **이동 기록 탭**(§9.6): 배치 단위 타임라인(시각, 규칙/AI/확인/재구성, 개수, 되돌림 상태)을 보여 준다. 파일 목록을 펼쳐 볼 수 있고 배치별로 되돌릴 수 있다. CLI는 `tidy log --batches`, `tidy undo --batch ID`다.
  - 리포트·재구성 객체는 코어 JSON(snake_case)을 그대로 IPC로 보낸다(`src/ipc/types.ts`에 같은 모양으로 정의).

---

## 10. 개발 마일스톤

| 단계 | 산출물 | 완료 기준 |
|---|---|---|
| M0 | 워크스페이스, CI, `tidy-cli scan <dir>` | 트리·통계 텍스트 출력 |
| M1 | Watcher + Rules + Mover + Undo (CLI) | 바탕화면에 파일 생성 → 규칙대로 이동, `undo` 동작 |
| M2 | Store + Indexer + FTS 검색 (CLI) | `tidy-cli search "키워드"` 본문 포함 결과 |
| M3 | Tauri 앱: 트레이, 검색창, 확인 팝업, 대기열 | 상시 실행 상태에서 전체 플로우 |
| M4 | AI Layer: Provider 3종, 분류, 학습 루프 | 키 없을 때 완전 동작 + 키 있을 때 AI 제안 |
| M5 | Reporter UI + AI 재구성 diff/적용 | 배치 Undo 포함 |
| M6 | NAS 폴링·복사-검증 이동·메타 인덱싱 | 마운트 끊김/복구 테스트 통과 |
| M7 | 임베딩 의미검색, 설치 패키지(win/mac/linux) | 1차 릴리스 |

각 마일스톤은 CLI로 먼저 검증한 뒤 UI를 붙인다.

---

## 11. 비기능 요구사항
- 메모리 상주 150MB 이하(인덱싱 중 제외), 유휴 CPU 1% 이하
- 10만 파일 인덱스에서 검색 응답 목표(5.6)
- 모든 파일 조작은 로그 + Undo 가능. 데이터 손실 0을 최우선.
- 크래시 시 재시작해도 Pending/Undo 로그 유지(WAL 모드)
- 오프라인 완전 동작(AI 제외)
- 경로·본문 등 개인 데이터는 로컬 DB 외부로 나가지 않음(AI 프로필이 허용한 범위 제외)

---

## 12. 테스트 전략
- 코어: 임시 디렉토리 기반 통합 테스트(생성→분류→이동→undo), 추출기별 샘플 파일 fixture
- Watcher: 폴링 경로는 가짜 네트워크 디렉토리로 시뮬레이션 (M6: 폴더 이름 변경으로 마운트 해제/재마운트를 흉내 냄. `WatchOptions.poll_override`/`reconnect`로 1초 미만 주기)
- AI: `MockProvider`로 결정론적 응답. 실제 API는 수동 테스트만.
- 크로스플랫폼 경로 처리(UNC, 한글 경로, 긴 경로) 단위 테스트 필수

---

## 13. 열린 질문 (구현 중 결정)
- ~~한국어 FTS 토크나이저: unicode61로 충분한지, trigram 병행이 필요한지 M2에서 벤치~~ → **결정(M2)**: trigram 주 + unicode61 보조(1~2글자 prefix). 근거와 측정치는 `docs/fts-tokenizer.md`.
- sqlite-vec vs 자체 brute-force: 파일 수 기준 임계치 정하기
- Linux 키 저장소: M4는 커널 keyutils(빌드 의존성 없음)를 쓰므로 재부팅하면 키가 사라진다. Linux 데스크톱 배포(M7) 때 Secret Service(dbus) 사용을 검토한다. Windows와 macOS는 영구 저장이다.
- `source_app`(어느 앱이 만든 파일인지) 수집 가능 범위: macOS는 `kMDItemWhereFroms`, Windows는 Zone.Identifier ADS — M1 후반에 조사
