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
  - dedupe: 안정화 단계가 마지막으로 보고한 (크기, mtime)을 1시간 기억하고, 같은 상태로는 다시 보고하지 않는다. 알림으로 이미 처리한 파일이 다음 목록에 나타나도, macOS FSEvents가 늦게 같은 이벤트를 다시 보내도 한 번만 처리한다. 재시도(requeue)는 이 기록을 지우고 다시 보고한다.
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
- `dest` 템플릿 변수: `{yyyy}` `{mm}` `{dd}` `{ext}` `{stem}` `{source_dir_name}` `{root}`(M8)
  - (M8) 상대 경로와 `{root}`는 감시 폴더의 정리 방식이 정하는 기준 폴더를 따른다: `in_place`면 감시 폴더, `collect`면 `organize_root`(§14.1). 아래의 "원래 파일이 있던 폴더 기준"은 감시 폴더 밖의 파일에만 해당한다.
  - (M8) 매치 조건 추가: `name_contains`(대소문자 무시 부분 일치), `newer_than_days`. 꺼진 규칙(`enabled = false`)도 로드 때 검사한다.
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
- **M7 구현 (search/semantic.rs)**
  - 켜지는 조건: `[ai] enabled`, `[search] semantic`(기본 true), 활성 프로필에 `embed` 모델. 하나라도 없으면 FTS만 쓴다(오류 아님).
  - 임베딩 텍스트: 파일명(`_ - .`을 공백으로) + 본문 앞 `search.embed_chars`(1,000)자. `send_content = false`면 파일명만 보내고, 임베더 키에 `#name`을 붙여 본문 포함 벡터와 섞이지 않게 한다.
  - 임베딩 시점: `tidy index`가 끝난 뒤(`--no-embed`로 끔), `tidy embed [--rebuild|--status|--limit N]`, `tidy watch`/앱의 백그라운드 스레드(백로그가 있으면 연달아, 없으면 30~60초마다). `search.embed_batch`(32)개씩 요청하고, 같은 텍스트는 한 번만 보낸다. 이미 같은 (모델, 텍스트 해시) 벡터가 있으면 요청 없이 연결만 한다. 참조가 없어진 벡터는 7일 뒤 지운다(그 사이 옮겨진 파일이 재사용).
  - 질의: 필터를 뺀 단어들을 임베딩한다(제한 `search.query_timeout_ms` 1,500ms, 최근 32개 캐시). 필터(`ext:` `in:` 날짜·크기)는 벡터 후보에도 그대로 적용한다. 코사인 유사도가 `search.min_similarity`(0.3) 이상인 상위 `search.vector_k`(50)개를 FTS 상위 목록과 RRF(k=60)로 합친다. 벡터 가중치는 키워드형 0.7, 질문형 1.5다. 질문형은 `?`로 끝나거나, 4단어 이상이거나, "어디·언제·관련·작년·what…" 같은 단서가 있는 질의다.
  - 결과 표시: 단어가 하나도 맞지 않고 뜻으로만 찾은 파일은 `matched_in = semantic`(앱 "의미 NN%" 배지, CLI `[meaning 83%]`)이다. 스니펫은 본문 앞부분이다.
  - 폴백(CLAUDE.md 규칙 4): 모델이 없거나, 실패하거나, 시간을 넘기면 그 질의는 FTS 결과만 돌려주고 `note`에 이유를 담는다. 연속 실패하면 일시 중단한다(§5.8 AI 가용성).
  - 앱: 입력하면 단어 결과를 먼저(120ms) 보여 주고, 입력이 멈추면(450ms) 단어+의미 결과로 바꾼다. 하단에 모드(단어 / 단어+의미(질문형) / 폴백 이유)를 표시한다. 의미검색 질의는 자체 DB 연결을 쓴다(느린 임베딩 요청이 UI 연결을 잡지 않도록).
  - 성능(10만 × 768차원, release, 1코어): 메모리 캐시 적재 0.8s·73MB(int8), 질의 약 25ms. 측정 방법은 §13.

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
- **M7 구현 (임베딩·가용성)**
  - 실제 트레이트는 `embed(&[String]) -> Result<Embeddings { vectors, usage }, AiError>`와 `name()`, `model()`이다. 차원은 모델이 돌려준 대로 쓰고(`dim()` 없음), 같은 임베더 키의 벡터끼리만 비교한다.
  - 구현체: OpenAI 호환 `POST {base}/embeddings`(`index` 순서로 정렬), Ollama `POST {base}/api/embed`, 테스트용 `MockEmbedder`(단어·3글자 해시 + 동의어 묶음, 실패 전환 가능). Anthropic에는 임베딩 API가 없어 `embed`에 지정하면 `Unsupported`로 알리고 의미검색만 꺼진다.
  - `ai_log` 목적에 `embed`(인덱싱)·`search`(질의)를 추가했다. OpenAI `text-embedding-3-small/large` 단가를 비용 추정표에 넣었다.
  - **AI 가용성(`AiHealth`)**: 호출 결과를 세어 `ai.unavailable_after`(3)번 연속 실패하면 "사용 불가"가 된다. 그동안 파일은 AI를 건너뛰고(요청·타임아웃 없음, 규칙만) 그 경로를 기억한다. 60초 뒤부터 작은 요청으로 다시 확인하고, 실패하면 간격을 두 배로(최대 10분) 늘린다. 한 번 성공하면 "준비"로 돌아온다. AI가 실패했거나 건너뛴 파일(최대 1,000개 — 사용 불가가 되기 전에 실패한 파일 포함)은 다음 성공 뒤 한 번 더 AI에 보낸다(파일마다 한 번만). 거부(refusal)는 연결된 것으로 센다. 감시 루프는 `WatchEvent::AiHealth`로 알리고, 앱 상태줄은 "AI 사용 불가 — 규칙만" ↔ "AI 사용"으로 바뀐다. 의미검색도 같은 방식으로 자체 가용성을 가진다(상태줄 "의미검색 켜짐 / 일시 중단").
  - **Linux 키 저장소(§13 결정)**: Secret Service(GNOME 키링/KWallet, D-Bus)에 저장한다(`keyring`의 `sync-secret-service` + `crypto-rust` + `vendored` — libdbus를 정적으로 넣어 빌드 의존성이 없다). Secret Service가 응답하지 않는 환경(최소 WM, 헤드리스)에서는 커널 세션 키링(keyutils, 로그아웃 시 삭제)으로 폴백하고, `store_name()`이 현재 저장소를 알려 준다(`tidy ai status`, 앱 AI 설정 "키 저장 위치").
- **M4 구현**
  - 실제 트레이트는 `complete_json(system, user, schema: &serde_json::Value) -> Result<LlmResponse { value, usage, model }, AiError>`이다. 사용량과 응답 모델을 `ai_log`에 남기려고 위 스케치보다 정보를 더 돌려준다.
  - `EmbeddingProvider`는 선언만 해 두었고, 구현은 M7에서 한다(위 M7 항목).
  - Anthropic은 공식 Rust SDK가 없어 raw HTTP로 부른다. 요청은 `POST /v1/messages`, 헤더는 `x-api-key`와 `anthropic-version: 2023-06-01`이다. 답은 `output_config.format = {type: json_schema}`로 JSON만 받는다. 프로필의 `effort`(예: `"low"`)는 `output_config.effort`로 보낸다. `stop_reason = refusal`은 실패로 처리한다.
  - OpenAI 호환은 `/chat/completions`에 `response_format = json_schema(strict)`로 보낸다. 400/422가 오면 한 번만 `json_object` 모드로 다시 보낸다(스키마는 프롬프트에 넣는다).
  - Ollama는 `/api/chat`에 `format = 스키마`, `stream = false`, `temperature = 0`으로 보낸다.
  - HTTP는 `reqwest`(rustls/ring, OpenSSL·cmake 불필요)를 쓴다. 408/409/429/5xx(529 포함)와 네트워크 오류는 지수 백오프로 재시도하고(`retry-after` 존중, 최대 8s), 횟수는 `ai.max_retries`(기본 2)다. 400 같은 클라이언트 오류는 재시도하지 않는다. 별도 레이트 리미터는 아직 없다(429 재시도로 대신한다).
  - 엔진은 동기 코드라서 `AiClassifier`가 자체 current-thread tokio 런타임으로 `block_on`한다. 감시 스레드나 `spawn_blocking` 스레드에서만 부르고, async 컨텍스트 안에서는 부르지 않는다.
  - 키 저장: `keyring` 3을 쓴다. Windows는 자격 증명 관리자, macOS는 키체인, Linux는 커널 keyutils다(M7에서 Secret Service로 바꿈, 위 M7 항목). `key_ref = "service/user"`이고 `/`가 없으면 service는 `tidy`다. 키는 로그·설정·UI 응답 어디에도 나가지 않는다(`Debug` 출력도 가린다). 앱의 키 입력칸은 쓰기 전용이고, 화면에는 "저장됨" 여부만 보인다.
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

-- (M7) 임베딩: 벡터는 (모델, 임베딩한 텍스트의 해시)마다 하나만 저장하고
-- 파일은 그것을 가리킨다. 옮기거나 복사한 파일(이름·본문이 같으면)은 API를
-- 다시 부르지 않고 벡터를 재사용한다. sqlite-vec 대신 BLOB(§13 결정).
CREATE TABLE vectors (
  id INTEGER PRIMARY KEY,
  model TEXT NOT NULL,          -- 임베더 키 "provider/model[#name]"
  text_hash TEXT NOT NULL,      -- blake3(임베딩한 텍스트) 앞 32자
  dim INTEGER NOT NULL,
  vec BLOB NOT NULL,            -- f32 little-endian, L2 정규화
  created_at INTEGER NOT NULL,
  UNIQUE (model, text_hash)
);
CREATE TABLE file_vectors (
  id INTEGER PRIMARY KEY AUTOINCREMENT,   -- 계속 증가: 검색 캐시가 새 행만 읽는다
  file_id INTEGER NOT NULL UNIQUE REFERENCES files(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  text_hash TEXT NOT NULL,
  src_indexed_at INTEGER NOT NULL         -- 이 값이 files.indexed_at과 다르면 다시 임베딩
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

-- (M9) v7: 개인정보 판정·암호화 보관
-- files.pii_flags INTEGER   -- NULL = 판정 불가/미판정, 0 = 없음, 비트 1 주민번호 · 2 계좌 · 4 이름+휴대전화 · 8 이름+이메일
-- files.pii_counts TEXT     -- JSON {"rrn":2,"account":0,"name_phone":15,"name_email":0}
-- files.vault INTEGER       -- 1 = 경로가 암호화 zip(본문은 잠그기 전 것)
-- move_log.op TEXT          -- move | encrypt
-- move_log.dst_hash TEXT    -- encrypt: 만든 zip의 해시 (hash는 원본 해시)
-- pending.encrypt INTEGER   -- 승인하면 암호화
-- pending.pii TEXT          -- 탐지 결과 JSON (배지)

-- (M10) v8: 업무 맥락 분석 캐시 (§14.3)
CREATE TABLE curation (
  key TEXT PRIMARY KEY,        -- "<blake3>|<소문자 파일 이름>" 또는 "meta:<경로>|<크기>|<mtime>"
  path TEXT NOT NULL,          -- 마지막으로 본 경로(적용하면 새 경로)
  entity TEXT,                 -- NULL = 고객사 없음
  issue TEXT NOT NULL,
  issue_date TEXT,             -- YYYY-MM-DD (파일 자기 날짜)
  doc_kind TEXT NOT NULL,
  topic TEXT,
  confidence REAL NOT NULL,
  reason TEXT NOT NULL,
  source TEXT NOT NULL,        -- ai | user
  model TEXT,
  analyzed_at INTEGER NOT NULL
);
CREATE INDEX idx_curation_group ON curation(entity, issue);
```

마이그레이션: `schema_version`에 적용 버전을 기록하고 순서대로 적용. M1(v1)은 `move_log`, `pending`, `ignored`, M2(v2)는 `roots`, `files`, `files_fts`, `files_fts_word`를 만든다. M4(v3)는 `pending.source/suggested_name` 열과 `decisions`, `ai_log`, `learn_dismissed`를 추가한다. M5(v4)는 `restructure_plans`를 추가한다. M6(v5)는 `move_log.state`와 `dst_path`·`state` 인덱스를 추가한다(기존 행은 `done`). M7(v6)은 `vectors`, `file_vectors`를 추가한다. M9(v7)은 `files.pii_flags/pii_counts/vault`, `move_log.op/dst_hash`, `pending.encrypt/pii`를 추가한다. M10(v8)은 `curation`을 추가한다.

**DB 파일 위치**: SQLite는 파일 잠금이 필요하므로 `store.path`는 반드시 로컬 디스크여야 한다. NAS/공유 폴더/네트워크 드라이브에 두면 `disk I/O error`로 실패한다(에러 메시지에 힌트를 붙인다. M6: `kind = "network"` 루트 아래이거나 네트워크 위치로 보이는 store.path는 기동 시 경고 — CLI `# warning:`, 앱 활동 탭 "주의").

---

## 7. 설정 파일 (`config.toml`)

```toml
[general]
language = "ko"
hotkey = "CmdOrCtrl+Shift+Space"
quiet_mode = false
start_on_login = true          # (M7) 로그인 시 트레이로 자동 시작 (릴리스 빌드)
onboarded = true               # (M8) 첫 실행 마법사 완료. 없으면: [[watch]]가 있으면 완료로 봄 (§14.1)

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
mode = "collect"               # (M8) in_place(기본) | collect
organize_root = "D:/정리"      # (M8) collect: 상대 경로 규칙·{root}가 가리키는 기준 폴더

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

unavailable_after = 3          # (M7) 연속 실패 N회 → 사용 불가(규칙만), 성공하면 복귀

[search]                       # (M7) 의미검색 — 프로필에 embed 모델이 있을 때만
semantic = true
query_timeout_ms = 1500        # 질의 임베딩 상한, 넘으면 단어 검색만
vector_k = 50
min_similarity = 0.3
embed_batch = 32
embed_chars = 1000             # 파일명 뒤에 붙이는 본문 글자 수

[report]                       # (M5)
protected = []                 # 재구성이 절대 건드리지 않는 폴더
stale_days = 90
top_n = 10
max_moves = 500                # AI 재구성 한 번에 받는 이동 상한
# scan_depth = 3               # (M7) 큰 폴더: 이 깊이까지만 스캔 (`tidy report --scan-depth N`)

[security]                     # (M9, §14.2) 비밀번호는 여기 없다 — OS 키체인 tidy/vault에만
mode = "rules"                 # rules: 규칙 encrypt 옵션만(모드 B) | vault: 개인정보면 vault_dir로 먼저(모드 A)
vault_dir = "D:/정리/보안"
approval = "confirm"           # confirm(기본): 확인 팝업·대기열에서 승인 | auto: 묻지 않고 암호화
open_ttl_minutes = 10          # 검색에서 연 복호화 사본을 지우는 시간

[security.pii]
enabled = true
rrn = true                     # 주민등록번호
account = true                 # 계좌번호(근접어·은행명 동반)
name_phone = true              # 이름 + 휴대전화 (같은 줄/행)
name_email = true              # 이름 + 이메일 (같은 줄/행)
min_rrn = 1                    # 항목별 임계치(건수)
min_account = 1
min_name_phone = 1
min_name_email = 1
exclude = ["^010-?0000-?0000$"]  # 값(그대로 또는 숫자만)이 맞으면 세지 않음
ai_check = false               # AI가 켜져 있으면 정규식 양성을 AI에 한 번 더 묻기

[curate]                       # (M10, §14.3) 업무 맥락 정리 — AI 필요
enabled = false                # 감시 중 새 파일도 맥락으로 정리 (리포트의 "업무 맥락으로 정리"는 꺼져 있어도 됨)
prefer = false                 # 맥락 정리 우선: 규칙보다 먼저
context = "context.toml"       # 맥락 사전 (고객사·별칭, 이슈 유형, 자료 종류)
template = "{entity}/{yyyymmdd}_{issue}/{doc_kind}"
other_template = "기타/{topic}"
min_confidence = 0.6           # 미만은 미분류(그대로 둠)
merge_days = 14                # 같은 고객사·이슈를 이 일수 안이면 가장 이른 날짜 폴더로
batch_size = 30                # 한 번에 AI에 묻는 파일 수
excerpt_chars = 400            # 파일당 본문 앞부분(ai.send_content일 때만)

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
- **M8 추가** (§14.1)
  - 창 4개: `wizard`(첫 실행 마법사, `#/wizard`)가 추가됐다. 첫 실행은 `onboarded`로 판정하고, 트레이 좌클릭·두 번째 실행도 설정이 끝나기 전에는 마법사를 연다. 감시는 마법사를 마친 뒤에만 돈다(상태줄 "설정 마법사를 마치면 감시를 시작합니다" + "마법사 열기").
  - 설정 탭: 감시 폴더(정리 방식 열 추가) → 규칙 카드 → 접힌 "고급: rules.toml 직접 편집" → AI → DB → 기타("마법사 다시 실행"). rules.toml이 없을 때 에디터는 설명 주석만 보여 준다(예시 규칙을 자동으로 넣지 않는다).
  - 리포트 탭은 `main:report?plan=rules&path=…`로 열리면 그 폴더의 규칙 정리 미리보기를 바로 만든다(마법사의 "기존 파일 미리보기").
- **M9 추가** (§14.2)
  - 설정 탭: AI 다음에 "보안(개인정보)" 섹션(탐지 항목·임계치·제외 패턴·AI 2차 확인, 보관 방식, 보안 폴더, 승인 방식, 비밀번호 두 번 입력 + 키체인 저장소·이전 비밀번호 수). 저장은 탭 아래 **저장 버튼 하나**로 한다(섹션 저장 버튼 없음, #8). 비밀번호가 다르거나 8자 미만이면 저장 버튼이 꺼진다.
  - 규칙 카드 옵션 "🔒 개인정보면 암호화"(`encrypt = "pii"`)와, 켰을 때 "개인정보가 아니어도 항상 암호화"(`"always"`).
  - 확인 팝업·대기열: "개인정보: 주민번호 2 · 이름+휴대전화 15" 배지, "🔒 암호화해서 보관" 표시, 승인 버튼은 "암호화해서 이동".
  - 검색: 보관 zip에 🔒 배지와 미리보기 안내, 암호화되지 않은 개인정보 파일에 "개인정보" 배지. "열기"는 임시 사본을 풀어 연다.
  - 리포트: 카드 "개인정보 의심 (미암호화)"와 "개인정보 의심 파일 N개(미암호화 M개)" 한 줄.
- **M7 추가**
  - 검색창 의미검색(§5.6 M7), 상태줄 "의미검색 켜짐 / 일시 중단", AI 설정의 "의미검색(임베딩 모델)" 칸(제공자·모델·주소·키 이름·API 키, 키 저장 위치 표시).
  - **자동 시작**: 설정 "로그인 시 자동 시작"(`general.start_on_login`, 기본 켜짐). `tauri-plugin-autostart`로 Windows `HKCU\…\Run`, macOS LaunchAgent, Linux `~/.config/autostart/Tidy.desktop`에 등록한다. 앱이 시작될 때와 설정을 저장할 때 OS 상태를 설정값에 맞춘다. 등록 명령에는 `--autostart`가 붙고, 이렇게 시작하면 창 없이 트레이에만 있는다. 개발 빌드는 `target/debug` 경로가 등록되지 않게 막는다(테스트는 `TIDY_DEV_AUTOSTART=1`).
  - **패키징**: `tauri build` — Windows `.msi`(WiX ko-KR)·`.exe`(NSIS, 사용자 단위), macOS universal `.dmg`, Linux `.deb`·`.AppImage`. `.github/workflows/release.yml`이 `v*` 태그에서 만들어 초안 릴리스에 붙이고, 릴리스 바이너리에 개발용 테스트 브리지 흔적이 없는지 검사한다. 로컬 빌드와 수동 확인 절차는 `docs/packaging.md`.

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
| M8 | 첫 실행 마법사, 시각 규칙 편집기, 폴더별 정리 방식(§14.1) | 2차 |
| M9 | 개인정보 탐지 + 암호화 보관(§14.2) | 2차 |
| M10 | 업무 맥락 정리(고객사/이슈 묶음) + 설정 화면 정돈 + 사용 설명서(§14.3, §14.4) | 2차 |

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
- ~~sqlite-vec vs 자체 brute-force: 파일 수 기준 임계치 정하기~~ → **결정(M7)**: 자체 brute-force. 벡터는 SQLite BLOB(f32)로 저장하고, 검색은 메모리의 int8 사본(질의도 int8, i32 누적)을 전부 훑는다. 측정(`cargo test -p tidy-core --release bench_100k -- --ignored --nocapture`, 768차원, 1코어): 10만 개 질의 약 25ms·캐시 73MB(적재 0.8s), f32 그대로는 약 92ms·293MB. 목표(벡터 포함 300ms)의 1/10 수준이고 확장 모듈을 OS별로 배포할 필요가 없다. **다시 볼 임계치**: 임베딩된 파일이 50만 개를 넘거나(질의 >120ms·캐시 >360MB 예상, §11 상주 메모리 목표 초과) 1,536차원 이상 모델로 20만 개를 넘을 때 sqlite-vec(또는 ANN)를 검토한다.
- ~~Linux 키 저장소: M4는 커널 keyutils(빌드 의존성 없음)를 쓰므로 재부팅하면 키가 사라진다. Linux 데스크톱 배포(M7) 때 Secret Service(dbus) 사용을 검토한다.~~ → **결정(M7)**: Secret Service(D-Bus, libdbus 정적 포함)에 저장하고, 응답이 없으면 keyutils로 폴백한다(§5.8 M7). GNOME 키링(`dbus-run-session` + `gnome-keyring-daemon`)에서 저장·조회·삭제를 확인했다. Windows와 macOS는 그대로 영구 저장이다.
- `.hwp`(한글 5.0 바이너리) 본문 추출: OLE 복합 문서 + zlib 압축 레코드라 순수 Rust 구현이 크다. **2차 항목**으로 미룬다(지금은 메타데이터만 인덱싱, `.hwpx`는 ZIP+XML이라 별도 검토). 후보: `hwp` 크레이트 평가, 또는 `cfb` + 레코드 파서 자체 구현(본문 `HWPTAG_PARA_TEXT`만).
- `source_app`(어느 앱이 만든 파일인지) 수집 가능 범위: macOS는 `kMDItemWhereFroms`, Windows는 Zone.Identifier ADS — M1 후반에 조사


---

## 14. 2차 설계 (1차 릴리스 사용 피드백 반영)

1차를 실제 바탕화면에 설치해 본 결과: (a) 처음 켠 사람이 뭘 해야 하는지 알 수 없고, (b) 예시 규칙이 사용자 동의 없이 Documents로 보내는 것처럼 보이며, (c) 규칙을 TOML로만 쓸 수 있고, (d) 회사 문서의 개인정보 파일을 따로 안전하게 보관할 방법이 없다.

### 14.1 M8 — 첫 실행 마법사 · 시각 규칙 편집기 · 폴더별 정리 방식

**원칙**: 마법사가 끝나기 전에는 아무것도 옮기지 않는다. 예시 규칙은 자동 적용하지 않는다(사용자가 체크한 것만 규칙이 된다).

**첫 실행 판정**: `config.toml`이 없거나 `[general] onboarded = false`일 때. 설치판은 설치 과정에서 설정 파일이 생길 수 있으므로 파일 존재가 아니라 `onboarded` 플래그로 판정한다(1차 버그 수정).

**마법사 (3단계 + 마무리, 어느 단계든 "나중에")**
1. **감시 폴더**: 바탕 화면·다운로드 기본 체크, "다른 폴더…" 추가. 폴더마다 **정리 방식** 선택:
   - `in_place` **이 폴더 안에서 정리** — 하위 폴더를 만들어 그 안에서만 분류. 규칙 목적지는 이 폴더 기준 상대 경로(`문서/{yyyy}-{mm}`). 밖으로 나가지 않는다.
   - `collect` **지정 위치로 모으기** — 기준 폴더(`organize_root`, 예 `D:\정리`)를 고르면 그 아래에 분류 폴더를 만든다. 규칙 목적지는 `{root}/문서/{yyyy}-{mm}` 형태.
   - 폴더별로 다르게 설정 가능. `WatchRoot`에 `mode: in_place|collect`, `organize_root: Option<PathBuf>` 추가. 규칙 `dest`에 `{root}` 변수와 상대 경로(선행 `/`·드라이브 없음)를 허용하고, 상대 경로는 "파일이 있던 감시 폴더" 기준으로 푼다.
2. **시작 규칙 고르기**: 체크박스 목록(스크린샷 → 캡처 / 설치파일 → 설치파일(확인 후) / 문서 → 문서/연도-월 / 압축 → 압축 파일 / 사진·영상 / 기타 직접 추가). 체크한 것만 `rules.toml`에 쓴다. 각 항목에 "지금 감시 폴더에서 N개가 해당" 미리보기.
3. **AI (선택)**: 분류용 제공자/키, 의미검색용 임베딩. 건너뛰면 규칙만.
4. **마무리**: 요약(어디를 지켜보고, 어디로 보내는지) → "감시 시작". 그 뒤 "기존 파일 N개 중 규칙에 걸리는 M개를 미리보기로 볼까요?" → 리포트 탭의 재구성 미리보기로 연결(승인 전 무이동).

**시각 규칙 편집기 (설정 탭 기본 화면, TOML은 "고급"에 숨김)**
- 규칙 = 카드. **어떤 파일**: 확장자 칩(프리셋 그룹 + 직접 입력), 이름에 포함/정규식, 크기, 수정일. **어디로**: 탐색기형 폴더 트리(드라이브·즐겨찾기·감시 폴더에서 시작, 없는 폴더는 "새 폴더"로 즉시 생성 가능), 변수 칩(`{yyyy}` `{mm}` `{dd}` `{ext}` `{stem}` `{source_dir_name}` `{root}`). **옵션**: 옮기기 전 확인, 사용 안 함, (M9) 개인정보면 암호화.
- 드래그로 순서 변경(위가 우선). 카드마다 "지금 매치되는 파일 N개 · 예: …" 실시간 표시(`check_rules` 확장).
- 카드 ↔ `rules.toml` **양방향 동기화**: 카드 편집 시 TOML을 다시 생성하되 사용자가 TOML에 쓴 주석·순서·알 수 없는 키는 보존(규칙별 라운드트립, `toml_edit`). TOML 쪽을 직접 고치면 카드에 반영. 파싱 불가면 카드는 읽기 전용 + 오류 표시.
- **AI로 규칙 만들기**(AI 켠 경우): 감시 폴더 파일 목록(이름·확장자·크기·날짜만, 본문은 `send_content` 설정에 따름)을 보내 폴더 구조 + 규칙 초안을 받아 카드로 띄운다. 저장 전까지 아무것도 쓰지 않는다. 기존 재구성 엔진(§5.7)과 프롬프트를 공유한다.

**설정 탭 재구성**: 상단 "감시 폴더"(폴더별 정리 방식 포함) → "규칙"(카드) → 접힌 "고급: rules.toml 직접 편집" → AI → 데이터베이스 → 기타. "마법사 다시 실행" 버튼.

**M8 구현 확정 (구현 중 정한 세부)**
- **상대 경로·`{root}`의 기준(앵커)**: `in_place` 폴더는 감시 폴더, `collect` 폴더는 `organize_root`다. 그래서 같은 규칙(`문서/{yyyy}-{mm}`)이 두 방식 모두에서 그대로 쓰인다 — `{root}/문서/…`와 `문서/…`는 같은 뜻이다. `collect`인데 `organize_root`가 없으면 감시 폴더를 앵커로 쓰고 경고한다(`Config::warnings`). 어느 감시 폴더에도 속하지 않는 파일(`tidy classify`, 감시 밖 폴더의 재구성)은 M1처럼 파일이 있던 폴더가 앵커다. 하위 폴더까지 감시(`recursive`)하면 하위 폴더 파일도 감시 폴더(가장 깊은 것)를 앵커로 쓴다.
- 절대 경로 `dest`는 사용자가 쓴 그대로 따른다(M1~M7 규칙 호환). 카드 편집기는 앵커 밖으로 나가는 목적지에 "이 폴더 밖으로 옮깁니다" 경고를 붙인다. 마법사가 만드는 규칙은 모두 상대 경로라 밖으로 나가지 않는다.
- **`onboarded` 판정과 이전 버전**: `[general] onboarded`가 `true`면 끝난 것이다. 키가 아예 없으면 `[[watch]]`가 하나 이상 있는 설정(M1~M7에서 이미 쓰던 설치)만 끝난 것으로 보고, 감시 폴더가 없는 설정(설치 과정·다른 기능이 만든 파일)은 마법사를 띄운다. `onboarded = false`면 언제나 마법사다.
- **무이동 보장**: 감시 루프는 onboarded일 때만 돈다(상태줄 "설정 마법사를 마치면 감시를 시작합니다"). 마법사 단계의 선택은 화면에만 있고, "감시 시작"을 눌러야 `config.toml`(감시 폴더·정리 방식·`onboarded = true`)과 `rules.toml`(체크한 규칙만)을 쓴다. 3단계 AI 설정은 그 자리에서 저장되지만 감시가 멈춰 있으므로 아무것도 옮기지 않는다. 설정 탭에서 직접 감시 폴더를 저장하는 것도 명시적 설정이므로 `onboarded = true`가 된다. CLI `tidy watch`는 사용자가 직접 실행하는 명령이라 이 판정과 상관없이 돈다.
- **마법사 창**: 별도 창(`wizard`, `#/wizard`). 첫 실행, 트레이 좌클릭(아직 안 끝났을 때), 설정의 "마법사 다시 실행"에서 연다. 다시 실행할 때 기존 규칙은 그대로 두고 새로 체크한 시작 규칙 중 같은 이름이 없는 것만 뒤에 덧붙인다.
- **시작 규칙**(모두 상대 경로): 스크린샷 → `캡처/{yyyy}-{mm}` (png·jpg·jpeg + 이름 `^(스크린샷|Screenshot|Screen Shot|화면 캡처)`), 설치파일 → `설치파일`(확인 후), 문서 → `문서/{yyyy}-{mm}` (pdf·hwp·hwpx·doc·docx·xls·xlsx·ppt·pptx), 압축 → `압축 파일`, 사진·영상 → `사진·영상/{yyyy}-{mm}`, 기타 직접 추가(이름·확장자·폴더). "지금 N개 해당"은 항목마다 따로 센다(순서 무관). 카드 편집기의 매치 수는 실제 동작대로 "처음 맞는 규칙" 기준이다.
- **새 매치 키**: `name_contains`(대소문자 무시 부분 일치, 카드의 "이름에 포함"), `newer_than_days`(최근 N일 안에 수정). 카드의 "수정일"은 `older_than_days`/`newer_than_days`다.
- **라운드트립**: 카드는 `rules.toml`의 `[[rule]]` 표 하나에 대응하고 원래 위치(`origin`)를 기억한다. 저장하면 카드 순서대로 표를 다시 배열하되, 남은 표는 원래 표를 가져와 카드가 다루는 키(`name` `match.ext/name_regex/name_contains/min_size/max_size/older_than_days/newer_than_days/from_dir` `dest` `confirm` `enabled`)만 바꾼다. 주석, 키 순서, 카드가 모르는 키(`ai_auto`, `source_app`, 앞으로 생길 키)는 그대로 남는다. 파일 맨 앞 설명 주석은 첫 규칙이 바뀌어도 맨 앞에 남는다.
- **매치 미리보기**: 감시 폴더의 기존 파일(감시 설정의 하위 폴더 여부·무시 패턴 그대로, 최대 20,000개, 네트워크 폴더는 건너뛰고 표시)을 대상으로 규칙별 `count`·예시 3개·예시 목적지를 낸다. 앱은 `check_rules`, CLI는 `tidy rules check [rules.toml]`.
- **폴더 트리**: `list_dirs`(하위 폴더만, 숨김 제외, 펼칠 때만 읽음, 5초 넘으면 "응답 없음"), 시작점은 `fs_roots`(드라이브/`/`, 홈·바탕 화면·문서·다운로드, 감시 폴더, 기준 폴더). "새 폴더"는 `mover::create_dir`(파일시스템 변경은 mover만, CLAUDE.md 규칙 1)로 만든다.
- **AI로 규칙 만들기**: 재구성(§5.7)의 트리 요약(`summarize_tree`)을 감시 폴더마다 만들어 보내고 `{rules:[{name, ext[], name_contains, dest(상대), confirm, reason}]}`를 받는다. 목적지는 상대 경로만 받고(절대 경로·`..`는 거부), 받은 초안은 "AI 제안" 표시가 붙은 카드로 편집기에 들어갈 뿐 저장 전까지 아무것도 쓰지 않는다. `ai_log` 목적은 `rules`.
- (M9) 규칙 카드 옵션의 "개인정보면 암호화"는 M8에서는 자리만 있고 꺼져 있었다 → M9에서 켜짐(§14.2).

### 14.2 M9 — 개인정보 탐지 + 암호화 보관

**탐지 (`pii` 모듈, 오프라인·정규식)**: 인덱싱 때 이미 추출한 본문(§5.5)과 파일명을 대상으로 다음 중 하나면 `pii = true`:
- 주민등록번호 1건 이상 (`\d{6}-?[1-4]\d{6}` + 검증 자릿수)
- 계좌번호 1건 이상 (은행별 길이 패턴, 하이픈 포함 10~14자리; 전화·날짜와 구분하기 위해 "계좌|입금|예금주" 근접어 또는 은행명 동반)
- **이름 + 휴대전화** 동시 1건 이상 (같은 줄/셀에 한글 2~4자 이름 패턴과 `01[016789]-?\d{3,4}-?\d{4}`)
- **이름 + 이메일** 동시 1건 이상
- 항목별 켜고 끄기, 건수 임계치 조정, 제외 패턴(테스트 번호 등). AI가 켜져 있으면 "개인정보 여부"를 2차로 묻는 옵션(기본 꺼짐).
- 탐지 결과는 `files.pii_flags`(비트마스크)와 매치 건수로 저장하고, 검색 결과·리포트에 🔒 배지. 본문이 없는 파일(이미지·암호화된 xlsx)은 판정 불가로 표시.

**처리 (규칙 옵션 `encrypt` 또는 전역 "개인정보 보관" 규칙)**
- 모드 A **보안 폴더로 모으기**: `pii = true`면 다른 규칙보다 먼저 `security.vault_dir`(예 `D:\정리\보안`)로 이동하며 암호화.
- 모드 B **규칙 안에서 암호화**: 일반 규칙대로 목적지를 정하되 `encrypt = "pii"`(개인정보일 때만) 또는 `encrypt = "always"`.
- **승인 방식(설정)**: `security.approval = "auto" | "confirm"`. `auto`는 묻지 않고 암호화·이동, `confirm`은 확인 팝업(파일명·탐지된 항목 종류·건수 표시)에서 승인해야 진행. 기본 `confirm`.
- **포맷**: ZIP + AES-256(WinZip AE-2, `zip` crate `aes-crypto`). 파일 1개 → `<이름>.<확장자>.zip` 1개(안에 원본 1개). 반디집·7-Zip·알집에서 열린다(Windows 탐색기 기본 기능은 AES zip 불가 — 아래 확정 참조). 비밀번호는 설정에서 1회 지정 → OS 키체인(`tidy/vault`)에만 저장, 설정 파일·로그에 평문 금지. 비밀번호 변경 시 기존 파일은 재암호화하지 않고 "이전 비밀번호" 목록을 키체인에 함께 보관.
- **안전 절차 (mover 전용)**: 원본 해시 → 임시 zip 작성 → zip을 열어 복호화 스트림 해시가 원본과 같은지 검증 → 목적지로 이동 → 그제야 원본 제거. 중단 복구(§5.9)는 `.tidy-tmp`를 치우고 원본을 남긴다. `move_log`에 `op = "encrypt"`와 zip 경로·원본 해시 기록.
- **되돌리기**: 배치 undo 시 복호화해 원래 경로·이름으로 복원(zip은 제거). 비밀번호를 못 찾으면 zip을 그대로 두고 충돌로 보고.
- **검색**: 암호화 전 인덱스(본문 FTS·임베딩)는 유지하고 경로만 zip으로 갱신한다. 결과에서 열기 → 키체인 비밀번호로 임시 폴더에 풀어 연 뒤 닫히면 정리(`%TEMP%\tidy-open\`, 10분 후 삭제 — 이 삭제만 예외적으로 허용, 임시 사본 한정).
- **범위 밖**: 폴더 단위 암호화, 원본 문서 자체 암호(xlsx 비밀번호) 설정, 클라우드 동기화 폴더의 충돌 처리는 3차.

**M9 구현 확정 (구현 중 정한 세부)**
- **설정**: `[security] mode = "rules" | "vault"`(모드 B / 모드 A), `vault_dir`, `approval = "confirm" | "auto"`(기본 confirm), `open_ttl_minutes = 10`. `[security.pii]` `enabled`, 항목별 `rrn` `account` `name_phone` `name_email`(켜고 끄기)와 `min_*`(임계치, 기본 1), `exclude`(정규식 목록, 찾은 값 그대로 또는 숫자만으로 맞춰 봄), `ai_check`(기본 false). `mode = "vault"`인데 `vault_dir`가 없으면 경고하고 모드 B로 동작한다. 규칙 옵션은 `[[rule]] encrypt = "pii" | "always"`(없으면 암호화 안 함), 카드의 "(M9) 개인정보면 암호화" 토글이 `"pii"`를 쓴다.
- **탐지 세부**: 숫자는 하이픈으로만 이어진 덩어리 단위로 본다(앞뒤에 숫자가 붙으면 다른 번호). 주민번호는 `6-7`/`13`자리 + 월·일 + 성별 1~4 + 검증 자릿수(2020-10 이후 발급된 검증 자릿수 없는 번호는 놓친다 — 한계로 둔다). 계좌는 10~14자리·1~4덩어리(덩어리마다 2자리 이상)에서 휴대·지역번호 모양, 주민번호, 사업자번호(3-2-5)를 뺀다. 근접어: 같은 줄에 "계좌·입금·예금주·이체·송금·account" 또는 은행명, 또는 그런 말이 있는 머리글 줄 아래(빈 줄·다음 시트까지)의 표. 휴대전화는 `01[016789]`로 시작하는 10~11자리(하이픈 없음 또는 3-3/4-4), `+82-10-…`도 받는다. 이름은 흔한 성씨로 시작하는 3음절 낱말, 복성(남궁·황보·제갈·선우·독고·사공·서문·동방) 4음절, 또는 "이름·성명·성함·담당자·예금주…" 바로 뒤의 2~4음절 낱말이다. "연락처·이메일·고객님" 같은 일반 낱말과 "…세요·…니다·…에서" 같은 어미는 이름이 아니다. 건수: 주민번호·계좌는 찾은 개수, 이름+휴대전화/이메일은 줄마다 min(이름 수, 번호 수)의 합. 스프레드시트는 행 하나가 한 줄(셀은 공백으로 이어짐)이다.
- **판정 저장**: 인덱싱 때 본문으로 판정해 `files.pii_flags`(NULL = 판정 불가)·`pii_counts`에 둔다. 본문이 없으면 판정 불가지만 파일 이름만으로 양성이면 양성. M9 이전에 인덱싱된 행은 다음 인덱싱 때 저장된 본문으로 다시 판정한다(파일을 다시 읽지 않음).
- **엔진 순서**: Tidy가 놓은 파일 → (모드 A) 보안 폴더 안 파일은 그대로, 개인정보면 보안 폴더로 → 규칙(`encrypt` 반영) → AI. 엔진은 인덱스의 판정이 최신(크기·mtime 같음)이면 그것을 쓰고, 아니면 그 자리에서 본문을 읽는다. Undo로 돌아온 파일은 다시 잠그지 않는다. 승인이 필요하면(`approval = confirm`, 규칙의 `confirm`, **비밀번호가 아직 없음** — auto라도) 대기열(`pending.encrypt = 1`, 탐지 결과 배지)로 간다. 암호화해야 할 파일을 암호화 없이 옮기는 경우는 없다.
- **zip 세부**: 안의 항목 이름은 원래 파일 이름(이름 바꿔 승인하면 새 이름), zip의 수정 시각은 원본 것. 같은 이름 zip이 있으면 `이름.pdf (1).zip`. **정정**: Windows 탐색기의 기본 압축 풀기는 AES 암호화 zip을 열지 못한다(ZipCrypto만 지원) — 반디집·7-Zip·알집 같은 AES 지원 프로그램 또는 Tidy의 "열기"로 연다.
- **로그·복구**: `move_log.op = "encrypt"`, `hash` = 원본 해시, `dst_hash` = zip 해시(엔진의 "Tidy가 놓은 파일" 판단은 `dst_hash`로 한다). 중단 복구에서 원본과 zip이 둘 다 있으면: 원본이 기록 해시와 같을 때 우리 zip을 지우고 행을 철회(원본 보존), 다르면 둘 다 두고 "확인 필요"로 보고. `.tidy-tmp` 임시 zip은 기존 규칙대로 치운다.
- **되돌리기**: 현재 → 이전 비밀번호 순서로 열고, 임시 파일에 풀어 원본 해시와 같아야 제자리에 둔다(mtime 복원) — 그다음 zip을 지운다. 맞는 비밀번호가 없으면 zip을 그대로 두고 충돌로 보고(행은 활성으로 남아 재시도 가능).
- **비밀번호**: 8자 이상. 바꾸면 이전 것은 `tidy/vault-previous`(JSON 목록, 최대 20개)로 옮긴다. Linux에서 Secret Service가 없으면 커널 키링(로그아웃 시 삭제)에 저장되므로 설정 화면이 저장소 이름을 보여 준다. 커널 키링은 스레드에 세션 키링이 없으면 첫 조회에서 새로 만들며 그 조회는 빈 키링을 보므로, 키를 읽기 전에 한 번 준비해 다른 스레드·프로세스가 저장한 키(M7의 API 키 포함)도 찾게 했다. `TIDY_VAULT_KEYCHAIN`(기본 `tidy`)으로 키체인 서비스 이름을 바꿀 수 있다 — 헤드리스 GUI 테스트가 실제 보관 비밀번호를 건드리지 않게 쓴다.
- **검색·인덱스**: 암호화 직전에 원본을 인덱싱해 두고, 그 행을 zip 경로로 바꾼다(`files.vault = 1`, FTS·벡터 그대로). 이후 인덱싱은 vault 행을 다시 읽지 않는다. `tidy index --rebuild`는 zip의 본문을 잃는다(이름만 남음). 열기는 `<temp>/tidy-open/<uuid>/원래이름`에 풀고, 앱이 시작할 때와 1분마다 `open_ttl_minutes`가 지난 `<uuid>` 폴더만 지운다.
- **AI 2차 판단**(`ai_check`): 정규식 양성일 때만, 파일 이름·탐지 결과와(`send_content`일 때만) 숫자를 0으로·메일 아이디를 `***`로 가린 본문 일부를 보낸다. "개인정보 아님"이면 양성을 지우고, 실패하면 정규식 판정을 유지한다. `ai_log` 목적은 `pii`.
- **CLI**: `tidy pii scan <폴더> [--all] [--no-recursive] [--format json]`(읽기만), `tidy vault lock <파일> [--to 폴더]`, `tidy vault unlock <zip>`(그 zip을 만든 이동의 undo), `tidy vault status`, `tidy vault password`(설정·변경).

**UI**: 설정 → "보안(개인정보)" 섹션: 탐지 항목 체크, 보관 모드, 승인 방식, 보안 폴더, 비밀번호 설정/변경(키체인 저장 표시). 확인 팝업·대기열에 "개인정보: 주민번호 2 · 이름+휴대전화 15" 배지. 리포트에 "개인정보 의심 파일 N개(미암호화 M개)".

**CLI**: `tidy pii scan <폴더>`, `tidy vault lock <파일> / unlock <zip> / status`.

### 14.3 M10 — 업무 맥락 정리 (AI 큐레이션)

**문제**: 규칙·기존 AI 분류는 종류(확장자)별 정리에 머문다. 사용자는 "어느 고객사의 어느 이슈 때 생긴 자료"끼리 묶이길 원한다. 예: `바다정보/20261020_과금이슈/로그/…`, `바다정보/20261020_과금이슈/안내문서/…`. 고객사와 무관한 문서도 내용이 비슷한 것끼리 `기타/정책/문자 정책/…`처럼 주제로 묶는다.

**전제**: AI(분류용 제공자)가 켜져 있어야 한다. 꺼져 있으면 기능 자체를 "AI가 필요합니다" 배지와 함께 비활성으로 보여 주고, 켜는 방법을 안내한다. 임베딩은 선택(있으면 주제 묶기 정확도 향상).

**맥락 사전 (`context.toml`, 설정 → "업무 맥락")**
- `[[entity]]` 고객사/프로젝트: `name`, `aliases`(영문·약칭), `notes`(선택). `[[issue_type]]` 이슈 유형(과금, 장애, 전환, 점검, 제안…). `[[doc_kind]]` 자료 종류(로그, 안내문서, 계약, 회의록, 캡처, 데이터…)와 확장자 힌트.
- 처음엔 **"바탕화면에서 뽑기"**: 감시 폴더의 파일명 + 본문 앞부분을 AI에 보내 후보 고객사·이슈 유형을 추출 → 사용자가 체크·이름 수정 → 저장. 이후 수동 추가/별칭 편집.
- 사용자가 결과를 고치면(다른 고객사로 옮김, 이슈 이름 변경) 사전에 반영한다(§5.2 학습 루프와 같은 방식, 확인 후 저장).

**분석 (`curate` 모듈)**
- 입력: 파일명, 확장자, 크기, 수정일, 폴더, 본문 앞부분(인덱스에서, `send_content` 설정 존중), 맥락 사전.
- 출력(파일당, JSON 스키마): `entity | null`, `issue`(짧은 명사구), `date`(본문·파일명의 날짜 우선, 없으면 수정일), `doc_kind`, `topic`(고객사 없을 때 주제 경로 2단, 예 `정책/문자 정책`), `confidence`, `reason`.
- 배치(20~40개씩)로 묻고 결과를 `curation` 테이블에 캐시(파일 해시 기준, 바뀐 파일만 재분석). 확신도 < 임계치는 "미분류"로 남긴다. 같은 이슈를 날짜가 다르게 뽑은 파일들은 가장 이른 날짜로 묶는다(같은 entity+issue, 14일 이내).
- 비용 표시: 분석 전에 "파일 N개, 예상 토큰/비용" 안내.

**폴더 틀(설정, 기본값)**: 고객사 있음 `{entity}/{yyyymmdd}_{issue}/{doc_kind}/`, 고객사 없음 `기타/{topic}/`. 변수는 규칙 dest와 같은 문법. 틀은 감시 폴더의 정리 방식(in_place/collect)에 따라 상대/`{root}` 기준으로 푼다.

**제안·적용**: 리포트 탭 "업무 맥락으로 정리" → 기존 재구성 미리보기 UI(트리 diff + 항목 체크)에 표시. 항목마다 AI의 판단(고객사·이슈·근거)을 보여 주고, 드롭다운으로 고객사/이슈를 바꿀 수 있다(바꾸면 사전 학습). 승인 전 무이동, 적용은 한 배치, 되돌리기 가능(§5.4). 개인정보 파일(§14.2)은 같은 폴더로 가되 암호화 설정을 따른다.

**새 파일**: 감시 중 규칙에 안 걸린(또는 "맥락 정리 우선" 옵션이 켜진) 새 파일은 같은 분석으로 기존 `entity/이슈` 폴더에 넣는다. 새 고객사·새 이슈로 판단되면 확인 팝업("바다정보의 새 이슈 '대체발송 중복'(20261020)으로 폴더를 만들까요?"). 확신도 ≥ `auto_threshold`이고 기존 폴더일 때만 자동.

**CLI**: `tidy curate scan <폴더> [--dry-run]`, `tidy context list|add|alias`, `tidy curate apply <plan-id>`.

**테스트**: MockProvider로 결정론적 분석 결과를 돌려 묶기·날짜 병합·틀 적용·undo를 검증. 한국어 fixture(고객사 2곳 × 이슈 2건 + 기타 정책 문서 3건).

**M10 구현 확정 (구현 중 정한 세부)**
- **설정**: `[curate] enabled = false`(감시 중 새 파일에 쓸지 — 리포트의 "업무 맥락으로 정리"는 꺼져 있어도 AI만 있으면 쓸 수 있다), `prefer = false`(맥락 정리 우선: 규칙보다 먼저), `context = "context.toml"`(설정 파일 기준 상대 경로), `template = "{entity}/{yyyymmdd}_{issue}/{doc_kind}"`, `other_template = "기타/{topic}"`, `min_confidence = 0.6`(미만은 미분류), `batch_size = 30`, `merge_days = 14`, `excerpt_chars = 400`(본문은 `ai.send_content`일 때만). 틀 변수: `{entity} {issue} {doc_kind} {topic} {yyyy} {mm} {dd} {yyyymmdd} {root}`. 틀은 상대 경로만(절대·`~`·`..` 거부), AI가 준 값은 폴더 이름 하나로 다듬는다(구분자·Windows 금지 문자 제거, 60자). `{topic}`은 최대 2단. 기준 폴더는 감시 폴더의 정리 방식(in_place = 그 폴더, collect = `organize_root`).
- **사전 파일이 없으면** 시작 사전(이슈 유형 과금·장애·전환·점검·제안, 자료 종류 로그·안내문서·계약·회의록·캡처·데이터·발표와 확장자 힌트, 고객사 없음)으로 동작하고, 저장할 때 만든다. 별칭은 대소문자·공백을 무시하고 맞춘다 — AI가 별칭(`BADA`)으로 답해도 사전 이름(`바다정보`)으로 바꾼다.
- **캐시 키**: `"<blake3 해시>|<소문자 파일 이름>"`(내용이 같아도 이름이 다르면 다른 판단 — 이름이 큰 단서라서), 본문을 못 읽는 파일은 `meta:<경로>|<크기>|<mtime>`. 저장소 v8 `curation` 테이블. 사용자가 미리보기에서 고친 판단은 `source = user`로 덮어써 다시 묻지 않고, 확신도와 무관하게 분류된 것으로 본다. 적용하면 새 경로로 갱신.
- **날짜 묶기**: (고객사, 이슈)별로 이번 제안의 파일 날짜 + 캐시에 있던 날짜를 정렬해, 앞 날짜와 `merge_days` 이하로 이어지는 사슬을 한 묶음으로 보고 묶음의 가장 이른 날짜를 폴더 날짜로 쓴다(그래서 10-14·10-20·10-27은 모두 1014).
- **미분류**: 확신도 미만, AI 응답에 빠진 파일은 제자리(`from == to`)로 두고 미리보기에서 체크하지 않는다. 고객사를 직접 정해 주면 분류된다.
- **AI 실패**: 분석 실패는 오류로 보고하고 아무것도 옮기지 않는다(절대 규칙 4: 블로킹·추측 이동 없음). `ai_log` 목적은 `curate`(분석)·`context`(사전 뽑기).
- **제안**: `restructure_plans`에 `source = "context"`로 저장하고 기존 미리보기·적용·배치 undo를 그대로 쓴다. 항목마다 `context`(고객사·이슈·날짜·자기 날짜·종류·주제·확신도·근거·AI 원래 답·미분류·고침 여부). 고치면 그 제안 전체를 다시 배치(날짜 묶기 포함)하고 사전 학습 후보(새 고객사, AI가 쓴 모르는 이름 → 고른 고객사의 별칭, 새 이슈 유형)를 돌려준다 — "사전에 반영"을 눌러야 `context.toml`에 쓴다. 모드 A(보안 폴더)에서 개인정보 파일은 같은 맥락 폴더에 `.zip`으로 암호화(§14.2 안전 절차). 배치 이름은 "업무 맥락 #id".
- **감시 중 새 파일**: `enabled`이고 AI가 있으면, 규칙에 안 걸린 파일(`prefer`면 규칙보다 먼저)을 한 개짜리 배치로 분석한다. 목적지 폴더(또는 그 상위의 `날짜_이슈` 폴더)가 이미 있고 확신도 ≥ `ai.auto_threshold`일 때만 바로 옮기고(`decided_by = ai`, 규칙 이름 "업무 맥락"), 나머지는 확인 팝업 — 질문은 대기열 `reason` 앞에 둔다: "새 고객사 'X'의 'Y'(yyyymmdd) 폴더를 만들까요?", "바다정보의 새 이슈 '대체발송 중복'(20261021)으로 폴더를 만들까요?", "새 주제 'Z' 폴더를 만들까요?". 되돌린 파일은 다시 옮기지 않는다. 분석 실패·미분류면 그대로 둔다.
- **비용 안내**: 캐시에 없는 파일만 센다 — 파일 수, 요청 수, 입력 토큰(글자 수 ÷ 2.5 근사), 출력 토큰(파일당 70), 알려진 모델이면 달러 금액.
- **CLI**: `tidy curate scan [폴더] [--dry-run] [--recursive]`(비용 안내 후 분석 → 제안 저장, `--dry-run`은 비용만), `tidy curate apply <plan-id> [--only 1,3]`(= `restructure --apply`), `tidy context list`, `tidy context add <이름> [--alias A,B]`, `tidy context add <이름> --issue`, `tidy context add <이름> --kind [--ext log,txt]`, `tidy context alias <고객사> <별칭>`.

### 14.4 M10 — 설정 화면 정돈 + 사용 설명서

- 설정 탭을 **접히는 섹션** 6개로 재구성: ① 감시 폴더 ② 규칙 ③ 업무 맥락(AI) ④ AI 연결 ⑤ 보안(개인정보) ⑥ 고급(데이터베이스·단축키·자동 시작·마법사 다시 실행·TOML 직접 편집). 기본은 ①②만 펼침, 상태는 기억.
- AI가 있어야 동작하는 항목은 모두 **"AI 필요"** 배지 + AI가 꺼져 있으면 비활성 + "AI 연결에서 켜기" 링크.
- 어려운 항목마다 ⓘ 도움말(한 줄 + 펼치면 예시): 확신도 기준, 폴링 간격, 본문 인덱싱 시간대, 보관 모드, 승인 방식, 키체인 등. 전문 용어는 한국어로 바꾸거나 괄호 설명.
- 마법사에도 같은 도움말 사용. 마법사 3단계(AI)에 "업무 맥락 정리를 쓰려면 AI가 필요합니다" 안내.
- **사용 설명서** `docs/manual.md`(한국어): 설치 → 마법사 → 평소 사용 → 규칙 편집 → 업무 맥락 정리 → 개인정보 보관 → 검색 → 되돌리기 → 문제 해결(SmartScreen, AI 키, NAS). 스크린샷은 e2e가 찍는 캡처를 재사용. README는 기능 요약 + 설명서 링크 + 설치 파일 받는 법(Releases 또는 Actions 아티팩트).

**M10 구현 확정 (설정 화면)**
- 섹션 접힘 상태는 WebView `localStorage`(`tidy.settings.sections`)에 둔다 — 설정 파일에 쓰지 않는다. 접힌 섹션도 편집 중인 값은 유지되고, 저장 버튼은 하나(#8): 일반 설정 → AI → 보안 → 업무 맥락(사전 + `[curate]`) 순서로 바뀐 것만 저장한다.
- "AI 필요"는 상태바의 AI 상태가 사용 가능(`ready`/`starting`)이 아닐 때 비활성 + 배지 + "AI 연결에서 켜기"(설정 ④를 펼치고 그 위치로 이동; 다른 창에서는 메인 창을 연다). 대상: ③ 업무 맥락 전체, 리포트의 "업무 맥락으로 정리"·"AI 재구성 제안", 규칙의 "AI로 규칙 만들기". ③에는 AI가 켜져 있어도 배지를 붙여 AI를 쓰는 기능임을 알린다.
- ⓘ 도움말 문구는 `src/components/helpTexts.ts` 한 곳에 두고 설정·마법사가 함께 쓴다. 용어: "AI 분류" → "AI 연결", effort → "생각 깊이(낮음·보통·높음)", 폴링 → "확인 간격(초, 폴링)", 제외 패턴 → "제외할 값", 임베딩 → "의미검색 모델".
