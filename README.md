# Tidy

바탕화면·지정 폴더를 상시 감시해 규칙/AI 판단으로 정리하고, 본문까지 인덱싱해 검색하는 크로스플랫폼 데스크톱 앱.

**📖 사용 설명서: [`docs/manual.md`](docs/manual.md)** (설치 → 마법사 → 평소 사용 → 규칙 → 업무 맥락 → 개인정보 보관 → 검색 → 되돌리기 → 문제 해결)

## 기능 요약

- **자동 정리**: 감시 폴더에 새 파일이 생기면 규칙(확장자·이름·크기·날짜)대로 옮깁니다. 폴더마다 "이 폴더 안에서 정리" 또는 "지정 위치로 모으기". 규칙은 카드로 편집.
- **확인 팝업·대기열**: 애매한 파일은 묻고 옮깁니다(이동 / 다른 곳 / 이름 바꿔 / 건너뜀 / 무시).
- **삭제 없음 + 되돌리기**: 모든 이동은 기록되고, 묶음(배치) 단위로 되돌립니다. 파일을 지우지 않습니다.
- **검색**: `Ctrl+Shift+Space`로 파일 이름과 본문(pdf·docx·xlsx·pptx·txt…)을 찾습니다. 의미검색 모델이 있으면 자연어로.
- **AI (선택)**: 규칙에 안 맞는 파일의 자리 제안, 규칙 초안, 재구성 제안. 키는 OS 키체인에만. AI가 없어도 모든 기본 기능이 동작합니다.
- **업무 맥락 정리 (M10, AI 필요)**: `고객사/날짜_이슈/종류`(예: `바다정보/20261014_과금이슈/로그`)로 묶는 미리보기 → 고쳐서 → 승인 후 한 번에 적용. 고친 내용은 맥락 사전에 학습.
- **개인정보 보관**: 주민번호·계좌·이름+연락처를 오프라인으로 찾아 AES-256 zip으로 보관(승인 후, 되돌리기 가능).
- **리포트**: 폴더 트리·통계·중복·오래된 파일, 규칙/AI 재구성 미리보기.
- **NAS**: 폴링 감시, 끊김/복구, 복사→검증 후 이동.

## 설치 파일 받기

- **정식 배포판**: 이 저장소의 **Releases**에서 OS별 파일(Windows `.msi`/`-setup.exe`, macOS `.dmg`, Linux `.deb`/`.AppImage`)을 받습니다.
- **개발판**: **Actions → Release → 최근 실행 → Artifacts**(Actions 탭에서 *Run workflow*로 직접 만들 수도 있음).
- Windows에서 "PC 보호" 창이 뜨면 **추가 정보 → 실행**(코드 서명 전). 자세한 내용: [`docs/packaging.md`](docs/packaging.md)

## 개발 문서

- 설계: [`docs/SPEC.md`](docs/SPEC.md)
- 개발 규칙·진행 상태: [`CLAUDE.md`](CLAUDE.md)

## 빠른 시작

```
cargo build --workspace
cargo test --workspace
cargo run -p tidy-cli -- scan ~/Desktop --depth 2
cargo run -p tidy-cli -- scan ~/Desktop --format json > desktop.json
cargo run -p tidy-cli -- config
```

## 감시 + 자동 정리 (M1)

```
cp config.example.toml config.toml     # 감시 폴더 등
cp rules.example.toml rules.toml       # 정리 규칙
cargo run -p tidy-cli -- classify ~/Desktop/*        # 어떤 규칙이 걸리는지만 확인
cargo run -p tidy-cli -- watch --config config.toml --dry-run
cargo run -p tidy-cli -- watch --config config.toml  # 실제 이동 (Ctrl+C로 종료)
cargo run -p tidy-cli -- log --config config.toml    # 이동 기록
cargo run -p tidy-cli -- undo --last --config config.toml
```

- 모든 이동은 SQLite `move_log`에 기록되고 `tidy undo`로 되돌릴 수 있습니다. 파일을 삭제하지 않습니다.
- `confirm = true` 규칙에 걸린 파일은 옮기지 않고 대기열에 넣습니다(`tidy pending`).

## 인덱싱 + 검색 (M2)

```
cargo run -p tidy-cli -- index ~/Documents --config config.toml      # 메타 + 본문 인덱싱
cargo run -p tidy-cli -- index --config config.toml                  # 설정의 watch/index_only 루트 전부
cargo run -p tidy-cli -- index --status --config config.toml         # 인덱스 통계
cargo run -p tidy-cli -- search "과금 내역" --config config.toml
cargo run -p tidy-cli -- search '보고서 ext:pdf,docx after:2026-09 in:"~/Documents/업무"' --config config.toml
cargo run -p tidy-cli -- search "회의록" --format json --config config.toml
```

- 본문 추출: txt/md/csv/json/소스코드(UTF-8·UTF-16·CP949), html, pdf, docx, pptx, xlsx/xls/ods. 200MB 초과 파일과 추출 실패 파일은 메타(이름·크기·날짜)만 인덱싱됩니다.
- 증분: 다시 실행하면 크기·수정시각이 바뀐 파일만 다시 읽습니다. 사라진 파일은 인덱스에서 빠집니다.
- 검색 필터: `ext:` `in:` `after:` `before:` `size:>10mb` `type:image|doc|video|audio|archive|code`, `"구절"`.
- 한국어 토크나이저 결정(trigram + unicode61 보조): [`docs/fts-tokenizer.md`](docs/fts-tokenizer.md)
- `[store] path`(DB)는 반드시 로컬 디스크에 두세요. NAS·공유 폴더 위에서는 SQLite가 `disk I/O error`로 실패합니다.

## 데스크톱 앱 (M3)

트레이에 상주하면서 감시·정리를 하고, 글로벌 단축키(기본 `Ctrl+Shift+Space`)로 검색창을 엽니다. 확인 팝업, 대기열, 설정 화면이 있습니다.

```
cd apps/desktop
npm ci
npm run tauri dev        # 개발 실행
npm run tauri build      # 설치 파일 (Windows: MSI/NSIS)
```

- 필요한 것: Rust stable, Node LTS, (Windows) Visual Studio Build Tools의 "C++를 사용한 데스크톱 개발"과 WebView2 런타임.
- 설정 파일: `%APPDATA%\tidy\config.toml`(Windows 기본). `--config <경로>` 인자나 `TIDY_CONFIG` 환경변수로 바꿀 수 있습니다. 설정이 없으면 아무것도 감시하지 않고 설정 탭을 엽니다.

## AI 분류 (M4, 선택)

규칙에 맞지 않는 파일만 AI(Anthropic / OpenAI 호환 / Ollama)에 어디로 보낼지 물어봅니다.

- **자동으로 옮기는 경우**: 확신도가 `auto_threshold`(기본 0.9) 이상이고, 목적지가 `ai_auto = true` 규칙의 폴더 안일 때만입니다.
- **그 외**: 확인 팝업(대기열)으로 묻습니다.
- **AI를 쓸 수 없을 때**: 키가 없거나, 오프라인이거나, 오류가 나면 규칙만으로 동작하고 파일은 그대로 둡니다.

```
# config.toml의 [ai] enabled = true, 프로필 설정 후
cargo run -p tidy-cli -- ai key set tidy/anthropic   # 키는 OS 키체인에만 저장 (설정 파일에 쓰지 않음)
cargo run -p tidy-cli -- ai status                   # 사용 가능 여부 / 이유, 누적 토큰·비용
cargo run -p tidy-cli -- ai test ~/Desktop/파일.pdf   # 한 번 물어보기 (이동 없음)
cargo run -p tidy-cli -- watch --config config.toml   # 규칙 → AI → 확인
cargo run -p tidy-cli -- pending --move 3 --rename "새 이름"
cargo run -p tidy-cli -- learn                        # 같은 결정 3번 → 규칙 제안
cargo run -p tidy-cli -- learn --accept 1             # rules.toml 맨 앞에 추가
```

앱에서는 설정 > AI 분류에서 제공자, 모델, 키, 기준값을 정하고 "연결 테스트"로 확인할 수 있습니다. 학습 제안은 대기열 탭 위에 카드로 나타납니다.

## 리포트와 재구성 (M5)

```
cargo run -p tidy-cli -- report ~/Desktop                       # 트리·통계·중복·오래 안 쓴 파일
cargo run -p tidy-cli -- report ~/Desktop --format html > 리포트.html
cargo run -p tidy-cli -- restructure ~/Desktop --rules          # 규칙대로 정리하면? (AI 불필요)
cargo run -p tidy-cli -- restructure ~/Desktop --ai             # AI 재구성 제안 (before/after diff)
cargo run -p tidy-cli -- restructure --apply 2 --only 1,3       # 승인한 항목만, 한 배치로 이동
cargo run -p tidy-cli -- log --batches                          # 이동 기록
cargo run -p tidy-cli -- undo --batch <id>                      # 재구성 통째로 되돌리기
```

- 재구성 제안은 **승인 전에는 아무것도 옮기지 않습니다.**
- 적용하면 한 배치로 기록되므로 한 번에 되돌릴 수 있습니다.
- 파일을 지우지 않고, 비게 된 폴더도 그대로 둡니다.

앱에서는 리포트 탭과 이동 기록 탭에서 같은 기능을 쓸 수 있습니다.

## NAS·공유 폴더 (M6)

`[[watch]]`에 `kind = "network"`를 주면 NAS 폴더를 감시합니다. 예시는 `config.example.toml`에 있습니다.

- **폴링**: OS 변경 알림에 더해 `poll_interval_s`(기본 120초)마다 폴더 목록을 비교합니다. 알림을 안 보내는 NAS는 `poll_only = true`로 둡니다.
- **연결 끊김**: 그 폴더만 일시정지하고 60초마다 다시 확인합니다. 다시 연결되면 끊긴 동안 생긴 파일도 처리합니다.
- **복사-검증 이동**: 다른 볼륨으로 옮길 때는 임시 파일로 복사한 뒤 크기·해시를 검증하고, 그다음에 원본을 정리합니다. 긴 복사는 진행률을 보여 줍니다.
- **중단 복구**: 복사 도중 앱이 꺼지거나 연결이 끊겨도 `tidy watch` 시작 시(또는 `tidy recover`) 자동으로 정리합니다. 파일은 원래 자리 아니면 목적지, 둘 중 한 곳에 남습니다.
- **인덱싱**: 기본은 메타데이터만입니다. 본문은 `index_content = true`와 `content_index_window`(예: `"02:00-05:00"`)로 켭니다.
- DB(`[store] path`)는 NAS에 두면 안 됩니다. 그렇게 설정하면 시작할 때 경고합니다.

```
cargo run -p tidy-cli -- watch --config config.toml   # "# offline: …" / "# online: …" / 복사 진행률
cargo run -p tidy-cli -- recover                      # 중단된 이동 정리(기본: 15분 지난 것만)
```

## 의미검색과 설치 패키지 (M7)

활성 AI 프로필에 `embed` 모델(Ollama 또는 OpenAI 호환 — Anthropic에는 임베딩 API가 없음)이 있으면, 검색창에서 "작년에 받은 통신 요금 고지서"처럼 **자연어로** 찾을 수 있습니다. 파일명과 본문 앞부분을 벡터로 저장해 두고 단어 검색(FTS) 결과와 합칩니다.

- 임베딩 모델이 없거나, 느리거나, 실패하면 그 검색은 **단어 결과만** 보여 주고 이유를 표시합니다.
- 옮기거나 복사한 파일은 저장된 벡터를 재사용하므로 API를 다시 부르지 않습니다.
- AI가 연속 3번 실패하면 "AI 사용 불가 — 규칙만"으로 바뀌고, 다시 응답하면 자동으로 돌아옵니다.

```
cargo run -p tidy-cli -- index                                  # 인덱싱 뒤 임베딩까지 (--no-embed로 끔)
cargo run -p tidy-cli -- embed --status                         # 임베딩된 파일 수
cargo run -p tidy-cli -- search 작년 통신 요금 고지서 어디 있지     # 단어 + 의미 ([meaning 83%] 표시)
cargo run -p tidy-cli -- search 회의록 --fts                      # 단어만
cargo run -p tidy-cli -- report ~/Desktop --scan-depth 2        # 큰 폴더는 깊이 제한
```

설치 파일(Windows `.msi`/`.exe`, macOS `.dmg`, Linux `.deb`/`.AppImage`)은 `v*` 태그를 푸시하면 GitHub Actions가 만듭니다. 로컬 빌드, 자동 시작(로그인 시 트레이), 제거 방법은 [`docs/packaging.md`](docs/packaging.md)를 보세요. Linux에서 API 키는 Secret Service(GNOME 키링/KWallet)에 저장됩니다.

## 첫 실행 마법사와 규칙 카드 (M8)

처음 켜면 마법사가 열립니다(설정 파일이 있어도 `[general] onboarded`가 없으면). **마법사를 끝내기 전에는 아무 파일도 옮기지 않습니다.**

1. **감시 폴더**: 바탕 화면·다운로드 등을 고르고, 폴더마다 정리 방식을 정합니다 — *이 폴더 안에서 정리*(`문서/2026-10`처럼 하위 폴더) 또는 *지정 위치로 모으기*(`D:\정리` 아래로).
2. **시작 규칙**: 스크린샷·설치파일·문서·압축·사진·영상 중 체크한 것만 규칙이 됩니다. 항목마다 "지금 N개 해당"이 보입니다.
3. **AI(선택)**: 건너뛰면 규칙만.
4. **감시 시작** → 기존 파일 중 규칙에 걸리는 것을 리포트 탭 미리보기로 볼 수 있습니다(승인 전 무이동).

설정 탭의 규칙은 카드로 편집합니다(확장자 칩, 이름 포함, 크기·날짜, 폴더 트리, `{yyyy}` 같은 변수 칩, 드래그로 순서, 카드마다 "지금 N개"). `rules.toml`은 "고급"에서 직접 고칠 수 있고, 주석·순서·카드가 모르는 키는 그대로 남습니다.

```
cargo run -p tidy-cli -- rules check                      # 규칙별로 지금 걸리는 파일 수·예시 (이동 없음)
cargo run -p tidy-cli -- rules starters --toml documents  # 시작 규칙을 TOML로
```

## 개인정보 탐지와 암호화 보관 (M9)

인터넷 없이 본문·파일 이름에서 **주민등록번호, 계좌번호(근처에 '계좌·입금·예금주'나 은행명), 이름+휴대전화, 이름+이메일**(같은 줄·같은 행)을 찾습니다. 항목별 켜고 끄기·건수 기준·제외 패턴은 설정 → 보안(개인정보)에서 정합니다. 본문이 없는 파일(이미지 등)은 "판정 불가"입니다.

- **보관 방식**: *규칙 안에서 암호화*(규칙 카드의 "🔒 개인정보면 암호화", `encrypt = "pii" | "always"`) 또는 *보안 폴더로 모으기*(개인정보 파일은 다른 규칙보다 먼저 `security.vault_dir`로).
- **승인**: 기본은 확인 팝업(파일명·"개인정보: 주민번호 2 · 이름+휴대전화 15")에서 승인해야 진행. 자동으로 바꿀 수 있지만, 비밀번호를 정하기 전에는 언제나 묻습니다.
- **형식**: 파일 1개 → `<이름>.zip` 1개(AES-256). 반디집·7-Zip·알집 또는 Tidy 검색의 "열기"로 엽니다(Windows 탐색기 기본 기능은 AES zip을 열지 못함).
- **안전**: 원본 해시 → 임시 zip → 풀어서 해시 확인 → 제자리 → 그다음 원본 정리. 모두 이동 기록에 남고 **되돌리기**로 원래 자리에 풀립니다.
- **비밀번호**: OS 키체인(`tidy/vault`)에만. 바꾸면 이전 비밀번호를 함께 보관해 예전 zip도 열립니다. 설정 파일·로그에는 남지 않습니다.
- 암호화한 파일도 검색에 🔒로 나오고, 열면 임시 사본(`<temp>/tidy-open`, 10분 뒤 삭제)으로 엽니다. 리포트에는 "개인정보 의심 파일 N개(미암호화 M개)".

```
cargo run -p tidy-cli -- pii scan ~/Desktop [--all]          # 개인정보 의심 파일 목록 (읽기만)
cargo run -p tidy-cli -- vault password                      # 보관 비밀번호 설정/변경
cargo run -p tidy-cli -- vault lock <파일> [--to 폴더]        # 암호화해서 보관 (되돌리기 가능)
cargo run -p tidy-cli -- vault unlock <zip>                  # 원래 자리로 풀기
cargo run -p tidy-cli -- vault status
```

## 업무 맥락 정리와 설정 화면 정돈 (M10)

AI가 파일 이름·날짜·본문 앞부분(`send_content`일 때만)과 **맥락 사전**(`context.toml`: 고객사·별칭, 이슈 유형, 자료 종류)을 보고 "어느 고객사의 어느 이슈 자료인지"를 판단해 `{entity}/{yyyymmdd}_{issue}/{doc_kind}`로, 고객사가 없으면 `기타/{topic}`으로 묶습니다.

- **바탕화면에서 뽑기**: 감시 폴더에서 고객사·이슈 후보를 뽑아 → 고른 것만 사전에.
- **미리보기**: 리포트 탭 "업무 맥락으로 정리" → 분석 전 비용 안내 → 항목별 AI 판단·근거 → 고객사/이슈/종류 수정(사전 학습 제안) → **승인 후 한 배치 적용**, 되돌리기 가능.
- 같은 고객사·이슈는 14일 안이면 가장 이른 날짜 폴더로 묶고, 확신이 낮으면 미분류로 그대로 둡니다. 분석 결과는 파일 해시로 캐시해 다시 묻지 않습니다.
- **감시 중 새 파일**(`[curate] enabled`): 기존 폴더 + 확신도 ≥ `auto_threshold`면 자동, 새 고객사·이슈면 확인 팝업.
- 설정 탭은 접히는 6개 섹션(감시 폴더 / 규칙 / 업무 맥락 / AI 연결 / 보안 / 고급)과 ⓘ 도움말, AI가 필요한 기능에는 "AI 필요" 배지.

```
cargo run -p tidy-cli -- context list                          # 맥락 사전
cargo run -p tidy-cli -- context add 바다정보 --alias BADA,바다
cargo run -p tidy-cli -- context alias 하늘통신 SKY
cargo run -p tidy-cli -- curate scan ~/Desktop --dry-run       # 비용만 (파일 수·토큰·금액)
cargo run -p tidy-cli -- curate scan ~/Desktop                 # 분석 → 제안 저장 (이동 없음)
cargo run -p tidy-cli -- curate apply 3 --only 1,2,5           # 승인한 항목만 한 배치로
```
