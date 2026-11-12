# CLAUDE.md — Tidy (파일 자동 정리 프로그램)

## 프로젝트 개요
바탕화면·지정 폴더를 상시 감시해 파일을 규칙/AI 판단으로 정리하고, 본문까지 인덱싱해 검색하는 크로스플랫폼 데스크톱 앱.
**설계 기준 문서는 `docs/SPEC.md`다. 구현 전 반드시 읽고, 설계가 바뀌면 SPEC을 먼저 수정한다.**

## 스택
- 코어: Rust (`crates/tidy-core`), CLI (`crates/tidy-cli`)
- UI: Tauri 2 + React + TypeScript (`apps/desktop`)
- DB: SQLite + FTS5 (rusqlite bundled), 임베딩은 sqlite-vec 또는 BLOB
- AI: `LlmProvider` / `EmbeddingProvider` 트레이트 → anthropic / openai-compatible / ollama

## 절대 규칙
1. **파일시스템 변경은 `mover` 모듈만 한다.** 다른 모듈에서 rename/remove/copy 호출 금지.
2. **삭제 금지.** 삭제가 필요하면 OS 휴지통(`trash` crate)으로만 보낸다.
3. 모든 이동은 `move_log`에 기록되고 Undo 가능해야 한다. 기록 없는 이동 코드는 리뷰에서 반려.
4. AI가 없어도(키 없음/오프라인) 모든 기능이 규칙+검색 모드로 동작해야 한다. AI 호출 실패는 항상 폴백, 절대 블로킹 금지.
5. API 키는 OS 키체인(`keyring`)에만 저장. 코드·설정·로그에 키 문자열 노출 금지.
6. AI 재구성 제안은 사용자 승인 없이 적용하지 않는다.

## 실행 환경 주의
- 클라우드 세션(VM)에는 디스플레이와 GUI 라이브러리가 없다. Rust 코어·CLI는 빌드·테스트까지 하고, Tauri 앱(`apps/desktop`)은 **컴파일/타입체크까지만** 하고 실행 확인은 하지 않는다. 실행 확인을 못 한 경우 완료 보고에 반드시 "로컬에서 `npm run tauri dev`로 화면 확인 필요"라고 명시한다.
- **GUI 테스트는 테스트 브리지로 한다.** WebView2는 자동화 입력을 받지 않으므로 Tauri 창을 직접 클릭하지 말 것. 개발 빌드(`npm run tauri dev`)는 `127.0.0.1:1421`에 브리지(`src-tauri/src/devbridge.rs`)를 열고, 브라우저에서 `http://localhost:1420/`(검색창 `#/search`, 팝업 `#/confirm`)을 열면 같은 번들이 실행 중인 앱 백엔드로 명령·이벤트를 보낸다(`src/ipc/transport.ts`). 트레이·전역 단축키·팝업 위치만 사용자에게 확인 요청. 클라우드에서는 `apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev xvfb` 후 `apps/desktop/e2e/run-headless.sh`로 앱을 Xvfb에서 띄워 Playwright(`e2e/m3.mjs`)로 전체 GUI 플로우를 직접 돌릴 수 있다.
- 세션 시작 시 `cargo --version`을 확인하고 없으면 rustup으로 stable 툴체인을 설치한다. Tauri 빌드 의존성(webkit2gtk 등)이 없어 데스크톱 크레이트 컴파일이 실패하면 `tidy-core`/`tidy-cli`만 검증하고 그 사실을 보고한다.

## 개발 순서
`docs/SPEC.md` §10 마일스톤 순서를 따른다. 각 단계는 **CLI로 먼저 검증**한 뒤 UI를 붙인다.
현재 단계는 아래 "진행 상태"에 기록한다.

## 코딩 규칙
- Rust: `cargo fmt`, `cargo clippy -- -D warnings` 통과. 에러는 `thiserror`로 모듈별 타입 정의, 상위에서 `anyhow`.
- 비동기: `tokio`. 파일 IO가 무거운 작업은 `spawn_blocking`.
- 경로는 항상 `PathBuf`/`Utf8PathBuf`로, 문자열 결합 금지. Windows UNC·한글 경로 테스트 포함.
- 설정은 `config.toml` → `Config` 구조체(serde). 기본값은 `Default` impl에.
- 프론트: 함수형 React, 상태는 zustand, Tauri IPC는 `src/ipc/*.ts`에 타입과 함께 모음.
- 커밋 메시지: `feat(core/watcher): ...`, `fix(desktop/search): ...` 형태.
- 버전: 마일스톤 머지 전에 `Cargo.toml`(workspace)·`apps/desktop/package.json`·`src-tauri/Cargo.toml`·`tauri.conf.json` 다섯 곳(`package-lock.json` 포함)의 버전을 함께 올린다(기능 추가 = minor, 수정만 = patch). 설치 파일명·앱 정보에 그대로 보인다.
- PR: **마일스톤당 PR 하나**(브랜치도 마일스톤별). 이전 마일스톤 PR이 아직 머지 전이면 그 브랜치를 base로 쌓는다(stacked). 수동 테스트 절차는 해당 마일스톤 PR에만 적는다.

## 테스트
- 코어 통합 테스트는 `tempfile` 디렉토리에서 생성→분류→이동→undo 전체 플로우를 돌린다.
- AI 테스트는 `MockProvider`만 사용. 실제 API 호출 테스트는 작성하지 않는다.
- 새 Extractor 추가 시 `tests/fixtures/`에 샘플 파일과 기대 텍스트를 함께 추가.

## 자주 쓰는 명령
```
cargo build --workspace
cargo test --workspace
cargo run -p tidy-cli -- scan ~/Desktop
cargo run -p tidy-cli -- watch --config ./config.toml   # --dry-run, --process-existing
cargo run -p tidy-cli -- undo --last                     # --batch <id>, --id <n>
cargo run -p tidy-cli -- log / pending / classify <파일>
cargo run -p tidy-cli -- pending --move <ID> [--to DIR] / --skip <ID> / --ignore <ID>
cargo run -p tidy-cli -- index <폴더 또는 파일>   # --rebuild는 전체 인덱스 삭제 후 재생성
cargo run -p tidy-cli -- search "키워드"          # 임베딩 모델이 있으면 단어+의미, --fts는 단어만
cargo run -p tidy-cli -- embed [--status | --rebuild | --limit N]   # 의미검색 벡터 (index가 끝나면 자동)
cargo run -p tidy-cli -- ocr [--status | --retry | --limit N | <파일>]   # 사진·스캔 PDF 글자 인식(M15, watch 중 자동). 클라우드: apt install tesseract-ocr tesseract-ocr-kor poppler-utils
cargo run -p tidy-cli -- vision [--status | --retry | --limit N | <사진>]   # 사진 내용 인식(M16, AI 비전, 기본 꺼짐 [vision] enabled, 하루 한도, watch 중 자동)
cargo run -p tidy-cli -- ask "지난달 받은 PDF를 프로젝트 폴더로 옮겨줘" [--rules]  → ask --apply <plan> [--only 1,3]   # 자연어 명령(M17, 승인 전 무이동)
cargo run -p tidy-cli -- serve [--listen 0.0.0.0:7841 --data-root /data --print-token]   # NAS 에이전트(M18, TIDY_TOKEN·TIDY_CONFIG), 이미지: docker build -f docker/agent.Dockerfile -t tidy-agent . / PC 쪽: tidy nas status|search <q>|pending|log
cargo run -p tidy-cli -- ai status / ai key set tidy/anthropic / ai test <파일> / ai log
cargo run -p tidy-cli -- learn [--accept N | --dismiss N]   # 학습 규칙 제안
cargo run -p tidy-cli -- rules check [rules.toml] [--each]   # 규칙별 지금 매치되는 파일 수·예시 (이동 없음)
cargo run -p tidy-cli -- rules starters [--toml documents,archives]   # 마법사 시작 규칙 목록 / TOML로 출력
cargo run -p tidy-cli -- report [폴더] [--format text|json|html] [--scan-depth N]
cargo run -p tidy-cli -- restructure [폴더] --rules|--ai  → --apply ID [--only 1,3]   # 승인 전엔 아무것도 안 옮김
cargo run -p tidy-cli -- review [폴더] [--ai | --rebuild | --export 목록.json]   # --rebuild = 처음부터 새로 짜기(0.16.0, 고객사 파일은 어디 있든 고객사/종류). 정리 위치 점검(M19): 나뉜 고객사·이슈 폴더 합치기·기타 속 고객사 폴더·작은 폴더 접기·맨 위 파일(규칙), --ai면 고객사 폴더 밖 고객사 파일도 (계획만, restructure --apply)
cargo run -p tidy-cli -- compact [폴더] [--min N]   # 정리 위치 다듬기(0.13.0): Tidy가 만든 작은 종류·주제 폴더를 한 단계 위로 (계획만, restructure --apply)
cargo run -p tidy-cli -- dedupe [폴더] [--apply [--only 1,3]]   # 내용이 같은 파일 그룹(정리 위치 사본 포함·기본 남김) → 승인 후 휴지통(M14)
cargo run -p tidy-cli -- empty [--apply [--only 1,3]]   # 빈 폴더 정리(0.14.0): 감시 폴더·정리 위치의 파일 없는 폴더 → 승인 후 지움, undo로 다시 만듦
cargo run -p tidy-cli -- log --batches / undo --batch <id>
cargo run -p tidy-cli -- recover [--grace-secs 0]        # 중단된 이동(복사 도중 종료·NAS 끊김) 정리
cargo run -p tidy-cli -- pii scan <폴더> [--all] [--format json]   # 개인정보 의심 파일 (읽기만)
cargo run -p tidy-cli -- vault password / status / lock <파일> [--to DIR] / unlock <zip>   # 암호화 보관(AES zip, 키체인 tidy/vault)
cargo run -p tidy-cli -- context list / add <이름> [--alias A,B | --issue | --kind --ext log] / alias <고객사> <별칭>   # 맥락 사전 context.toml
cargo run -p tidy-cli -- curate scan [폴더 | --all] [--dry-run]  → [curate settle <plan-id> [--by-kind]] → curate apply <plan-id> [--only 1,3]   # 업무 맥락 정리(AI, 승인 전 무이동). 하위 폴더는 통째로 한 항목(M12)
cd apps/desktop && npm ci && npm run tauri dev   # 클라우드: npm run build + (src-tauri) cargo clippy 까지만
cd apps/desktop && npx tauri build [--bundles deb,appimage]   # 설치 패키지 (docs/packaging.md), 태그 v*는 release.yml
S=/tmp/tidy-e2e bash apps/desktop/e2e/run-headless.sh m14    # 헤드리스 GUI e2e (m3~m17, m15는 tesseract 필요, m16은 가짜 LLM이 비전 모델 역할, m17은 자연어 명령(AI 없이), m14는 휴지통 = $T/휴지통 폴더(TIDY_E2E_TRASH), 공통 도우미 e2e/lib.mjs, 가짜 OpenAI 호환 서버 e2e/fake-embed.mjs, 느린 가짜 LLM e2e/fake-llm.mjs)
node apps/desktop/e2e/fake-llm.mjs 18095   # 느린·긴 가짜 Anthropic/OpenAI 호환 (파일당 1초, FAKE_MAX_TOKENS로 max_tokens 잘림 재현)
```

## 진행 상태
- [x] M0 워크스페이스 + `tidy-cli scan` (2026-10-15)
- [x] M1 Watcher + Rules + Mover + Undo (2026-10-15)
- [x] M2 Store + Indexer + FTS 검색 (2026-10-15) — 토크나이저 결정: docs/fts-tokenizer.md
- [x] M3 Tauri 앱 기본 플로우 (2026-10-15) — 실행 확인은 Windows 로컬에서
- [x] M4 AI Layer + 학습 루프 (2026-10-15) — 실제 API 확인은 로컬에서 (`tidy ai test`)
- [x] M5 Reporter + 재구성 diff (2026-10-15) — 리포트·규칙/AI 재구성(승인 후 한 배치 적용)·이동 기록
- [x] M6 NAS 폴링/복사-검증 이동 (2026-10-15) — 끊김/복구·중단 복구·메타 인덱싱. 실제 NAS 확인은 로컬에서
- [x] M7 의미검색 + 패키징 (2026-10-16) — 임베딩(OpenAI 호환/Ollama)·BLOB+int8 brute-force·RRF, AI 가용성, Linux Secret Service, 자동 시작, 설치 패키지(release.yml). 설치판 실행 확인은 Windows 로컬에서 (`docs/packaging.md`)
- [x] M8 첫 실행 마법사 + 시각 규칙 편집기 + 폴더별 정리 방식 (2026-10-19) — onboarded 플래그로 첫 실행 판정(마법사 전 무이동), in_place/collect·{root}, 규칙 카드↔TOML 라운드트립, 폴더 트리, AI 규칙 초안. 트레이·실제 창 확인은 Windows 로컬에서
- [x] M9 개인정보 탐지 + 암호화 보관 (2026-10-19) — 정규식 탐지(주민번호·계좌·이름+휴대전화·이름+이메일, 오프라인), 보안 폴더 모으기/규칙 encrypt, 확인/자동 승인, ZIP+AES-256(검증 후 원본 정리·undo 복호화), 키체인 비밀번호(이전 목록), 🔒 검색·임시 열기. 실제 압축 프로그램(반디집·7-Zip)·Windows 자격 증명 관리자 확인은 로컬에서
- [x] M10 업무 맥락 정리 + 설정 화면 정돈 + 사용 설명서 (2026-10-20) — context.toml 사전·바탕화면에서 뽑기, curate 배치 분석(해시+이름 캐시, store v8)·14일 날짜 묶기·폴더 틀, 리포트 미리보기 수정→사전 학습→한 배치 적용/undo, 감시 중 자동(기존 폴더+확신도)/확인 팝업, AI 필요 배지, 설정 6개 접는 섹션+ⓘ, docs/manual.md. 실제 AI 응답 품질·창 확인은 로컬에서
- [x] M11 UI 단순화 + 도움말/정보 + 정리 현황 (2026-10-23, 0.4.0) — 탭 홈/확인 대기/정리하기/되돌리기/설정, 상태바 요약+감시 현황 패널, 정리 현황 카드(미정리 수 백그라운드·캐시·20,000 상한)→지금 정리, 정리 위치 경고(두 번 확인), 절대 경로 규칙 경고+상대 경로로 바꾸기, 도움말 메뉴·정보 창·업데이트 확인(하루 한 번, check_updates)·로그 파일, release.yml 수동 실행=공개 릴리스. 트레이 정보 메뉴·실제 릴리스·실제 브라우저 열기는 로컬에서
- [x] M12 폴더 단위 업무 맥락 정리 + 경로 맥락 + 고객사별 찾기 (2026-10-23, 0.5.0) — 하위 폴더 = 한 항목(FolderUnit 요약·캐시 키, `{folder}`, Mover::move_dir 한 배치·빈 폴더 정리·undo), AI에 상대 경로(맨 위 폴더 = 고객사 단서), 감시 중 맥락 정리는 최상위 파일만·폴더는 미리보기 승인만, folder_settle_secs, 파일별로 나누기·날짜 수정, 홈 "새 폴더 N개"·고객사별 보기, 검색 고객사 필터, 팝업 "대기열에 두기". 실제 AI의 폴더 판단 품질·실제 NAS 폴더 이동·창 확인은 로컬에서

- [x] M13 화면 개편 (2026-10-26, 0.6.0) — 감시 폴더 별칭(`[[watch]] alias`)·PathText(가운데 줄임·전체 경로 title·클릭 열기), 홈 폴더 카드 4열+오늘 한 일(move_log `today_moves`)+고객사별, 확인 대기 5열 표↔900px 미만 카드·⋯ 메뉴 5개(팝업 같은 카드), 설정 왼쪽 메뉴 6쪽 `#/main/settings/<key>`(좁으면 가로 탭)·공통 정리 위치·폴더 편집 패널·규칙 한 줄 아코디언, 최소 창 960×640, 색 토큰, e2e `lib.mjs`+m13. 실제 창 크기·DPI·한글 글꼴·트레이·탐색기 열기는 로컬에서
- [x] M14 중복 파일 처리 + 한글 본문 검색 (2026-10-27, 0.7.0) — `[mover] duplicate_policy` 설정 카드(건너뜀·번호·휴지통, 이름만 같으면 언제나 번호), 건너뜀 사유 "똑같은 파일이 이미 …에 있어 그대로 둠"+기존 파일 열기, 확인 대기·팝업 미리 알림+세 버튼(`on_duplicate`), 리포트 중복 그룹(정리 위치 사본 포함·하드링크 제외·기본 남김)→미리보기→휴지통(`Mover::trash_duplicates`, move_log `op = trash`, undo는 휴지통 복원 안내, `TrashBackend`), `tidy dedupe`, hwp(cfb+deflate+PARA_TEXT, 암호=메타만, PrvText 폴백)·hwpx 추출기. 실제 Windows 휴지통·실제 한글 문서 여러 종은 로컬에서
- [x] M15 사진·스캔 PDF 속 글자 검색(OCR) (2026-10-28, 0.8.0) — Windows 내장 OCR(`Windows.Media.Ocr`, PDF는 `Windows.Data.Pdf`로 렌더링)·그 밖은 tesseract(+pdftoppm), 인덱서는 `files.ocr = pending`만 표시(store v10, 기존 이미지·스캔 PDF도 마이그레이션 때 대기), 백그라운드 `run_ocr_loop`가 본문으로 저장(단어·의미검색·개인정보 판정), 내용 해시 `ocr_cache`, `tidy ocr`, 설정 › 고급 카드·검색 "사진 속 글자" 표시, e2e m15. 실제 Windows OCR·한국어 언어팩 확인은 로컬에서(CI windows에서 tests/m15_ocr.rs가 실제 엔진으로 돎)
- [x] M16 사진 내용 인식(AI 비전) (2026-10-29, 0.9.0) — 분류 제공자로 사진 첨부(`complete_json_image`: Anthropic image 블록·OpenAI image_url·Ollama images), 1568px JPEG·EXIF 방향, store v11(`files.vision`·`caption`, `vision_cache`), 설명을 본문 끝 `[사진 설명]`으로(FTS·의미검색·분류·맥락 정리가 그대로 봄, OCR 글자는 앞에 유지), 백그라운드 `run_vision_loop`(최근 것부터·하루 한도·지금 못 씀=대기 유지), 새 사진은 분류 전에 설명(로컬 OCR로 개인정보 먼저 확인), send_content=false·개인정보·보관 파일은 안 보냄, `tidy vision`, 설정 › AI 카드·검색 "사진 설명", e2e m16. 실제 Claude/Ollama 비전 응답 품질·비용은 로컬에서
- [x] M17 자연어 명령 (2026-10-29, 0.10.0) — `command` 모듈: AI 해석(문장·날짜·폴더 이름만 전송, purpose=command) → 실패·없음이면 오프라인 규칙 해석기(기간·종류·출처·크기·옮길 곳·조사 떼기), 지우기·이름 바꾸기 거절, 인덱스 검색(최대 500) → `PlanSource::Command` 계획 저장 → 승인 후 `restructure::apply` 한 배치("명령 #N")·undo, 옮길 곳은 정리 위치·감시 폴더 3단계에서 이름으로(없으면 새 폴더 경고), `tidy ask`, 홈 맨 위 "말로 시키기" 카드, e2e m17. 실제 AI 해석 품질·창 확인은 로컬에서
- [x] M18 NAS에서 직접 정리 (2026-10-29, 0.11.0) — `tidy serve` 에이전트(`remote::server`, tiny_http, `/api/v1` Bearer 암호·401+1초, data root 밖 거부, 감시=`run_watch` 규칙만·인덱싱·OCR, 지금 정리=한 배치, 확인 대기·되돌리기·검색·폴더 트리·설정 PUT), Docker 이미지(tesseract+kor, amd64·arm64, ghcr + 릴리스 tar.gz), PC `[[nas]]`(암호는 키체인 `tidy/nas/<이름>`, `[[nas.map]]` 경로 바꾸기)·설정 › NAS(설치 파일 만들기→연결→NAS 폴더·규칙 저장)·홈 NAS 카드·검색 NAS 결과·NAS 확인 대기·NAS에서 한 일, inotify 한도 넘으면 폴링, e2e m18. 실제 시놀로지 설치·ghcr 공개 여부·ARM NAS는 로컬에서
- [x] 0.11.1 수정 (2026-11-02) — "지금 정리"가 규칙 0개(맥락 사전만)일 때 빈 규칙 미리보기에서 멈추던 문제: 바로 업무 맥락 비용 안내로, 남는 파일·폴더 수 안내+업무 맥락/규칙·AI 링크, 미리보기 안내 강화·적용 버튼 줄 고정(sticky)·"적용하면 (미리보기)", 홈 "적용하지 않은 미리보기 [이어서 보기]"(list_plans `moves`), e2e fix-organize
- [x] 0.11.2 수정 (2026-11-02) — 예전 예시 규칙의 절대 목적지(`~/Documents/Tidy/…` 등)를 시작 때 정리 위치 기준으로 자동 변환(백업 `rules.toml.bak-0.11.2`)+이미 옮긴 파일 "정리 위치로 옮기기" 계획(`report::relocate`), 제안 파일 열기(`report::import`, 옮기기만·root 안·폴더 통째·빈 부모 폴더 정리), e2e fix-relocate. SPEC §14.16
- [x] 0.11.3 수정 (2026-11-02) — 정리하기 화면 개편: 단계 표시 ①②③ + 큰 버튼 하나("정리 미리보기 만들기" = 홈 지금 정리와 같은 `start`), 나머지 방법은 "다른 방법으로 정리하기"에 접음, 미리보기 한 줄 요약, 미분류 한 번에 정하기(`Curator::settle_unclassified`: AI 추측대로 / 기타/미분류/{종류}, `tidy curate settle`), 0개일 때 옮기기 줄에 이유+버튼, [선택한 N개 옮기기], 적용 뒤 "남은 파일로 다시 미리보기", e2e fix-flow. SPEC §14.17
- [x] 0.12.0 업무 맥락 흐름 다시 짜기 (2026-11-02) — 설정 › 업무 맥락을 가져올 곳(감시 폴더)→만들 곳(정리 위치·기존 폴더 수) + ①사전("감시 폴더 N곳에서 뽑기") ②처음 전체 훑기 ③새 파일 순서로, 정리하기 "모든 감시 폴더 한 번에"(`Curator::plan_many`·`curate::all_sources`, `tidy curate scan --all`) + 폴더 구성안(`Preview.structure`, 기존/새 폴더), 정리 위치의 기존 폴더 재사용(`curate::existing`: 철자·띄어쓰기 무시, 14일 안의 같은 이슈 폴더, AI 프롬프트 힌트), 감시 중 새 파일도 같은 재사용·폴더로 고객사 인식, e2e fix-context. SPEC §14.18. 실제 AI 판단 품질·창 확인은 로컬에서
- [x] 0.12.1 수정 (2026-11-02) — "감시 폴더에서 뽑기"가 AI에 보내는 파일 이름 200개·폴더 이름 100개를 감시 폴더마다 고르게 나눔(`curate::fair_share`, 예전엔 첫 폴더부터 채워 파일 많은 바탕 화면이 다른 폴더를 밀어냄). SPEC §14.18
- [x] 0.12.2 수정 (2026-11-09) — 전체 점검(M1~M18 테스트·clippy·헤드리스 e2e 20개): 되돌리기 때 파일 이동이 새로 만든 빈 목적지 폴더(`문서/2026-11` 등)도 치움(`created_dirs`, 전엔 폴더 통째 이동만, SPEC §5.4), `tidy embed --status`가 AI 꺼짐이면 오류 대신 상태 표시, e2e m5·m9·m10·m11·m12를 0.11.3·0.12.0 화면(접힌 "다른 방법"·"모든 감시 폴더" 선택지)에 맞춤
- [x] 0.12.3 수정 (2026-11-09) — 바탕화면을 정리해도 폴더가 많이 남던 문제: 모으기 설정에서 예전에 바탕화면 안에서 정리했던 폴더(Tidy가 옮겨 넣은 파일이 있는 폴더)를 "정리 결과"로 보고 통째로 빼던 것 → 안의 파일을 하나씩 정리 위치로(`Curator::scan_folders`·`FolderScan.leftovers`, 적용 뒤 빈 폴더 정리), 미리보기 요약에 그대로 두는 항목 표시, 업무 맥락 표 열 너비 고정(옮길 곳·AI 판단이 창 밖으로 넘치던 문제). SPEC §14.7
- [x] 0.12.4 수정 (2026-11-09) — 0.12.3 뒤에도 홈 바탕화면 카드가 "0개 · 정리됨"(예전 정리 폴더를 세지 않음) → "정리할 폴더 N"+지금 정리, `~/Documents/tidy`(git 클론 = Windows에선 `Documents/Tidy`)의 crates·apps·.git을 "정리 위치로 옮기기"로 제안하던 문제 차단(`relocate::is_dev_project`, 숨김 폴더 제외, 시작 때 그런 미리보기 삭제), 홈 미리보기 [버리기](`discard_plan`). SPEC §14.7·§14.16
- [x] 0.12.5 수정 (2026-11-10) — 고객사 폴더 안 소스 트리가 낱개로 흩어지던 문제: 예전 정리 폴더(leftovers)는 Tidy가 만든 하위 폴더까지만 파일별로, 손대지 않은 하위 폴더(`고객사 로그/tools`)는 통째 항목, 일반 AI 분류도 하위 폴더 안 파일은 안 물음, 시작 때 확인 대기의 그런 예전 AI 제안 정리(`Engine::drop_scattering_pending`), AI 지시에 "고객사 폴더 안 코드·도구도 그 고객사". SPEC §14.7
- [x] 0.13.0 정리 구조 점검 (2026-11-10) — 설계 원칙(누구의 일 → 이슈 → 많을 때만 종류, 폴더는 내용 있을 때만, 같은 파일은 언제 와도 같은 곳) SPEC §14.19. `[curate] min_folder_items = 3`(새 자료 종류·작은 주제 폴더는 3개 이상일 때만, 아니면 한 단계 위·기존 폴더는 항상 사용, 감시 중 한 파일은 새 종류 폴더 안 만듦, "종류별로 모으기"는 예외), 파일 이름에 고객사(사전·별칭·캐시)가 보이면 감시 중 업무 맥락을 규칙보다 먼저(`Curator::customer_hint`, 실패·미분류면 규칙), AI 주제 이름 넓게·재사용 지시, 시작 규칙 문서 `문서/{yyyy}`, 설정 세부 "작은 폴더 만들지 않기", 이미 쌓인 작은 폴더는 "정리 위치 다듬기"(`report::compact`, 첫 시작 때 한 번 미리보기, `tidy compact`). 실제 AI 판단·창 확인은 로컬에서
- [x] 0.13.1 수정 (2026-11-10) — 0.13.0에서도 바탕화면에 폴더가 가득한데 홈 카드가 "0개 · 정리됨": 빠지는 하위 폴더마다 이유를 남김(`FolderScan.kept`, 제외 폴더·무시 패턴·건드리지 말 폴더·정리 위치/규칙 목적지 겹침·빈 폴더·링크 폴더 등), 홈 카드 "그대로 둔 폴더 N개 ▸ 왜?", 미리보기 요약·`tidy curate scan`·로그에도. SPEC §14.7
- [x] 0.14.0 빈 폴더 정리 (2026-11-12) — 감시 폴더·정리 위치에서 파일이 하나도 없는 폴더(빈 하위 폴더·Thumbs.db·desktop.ini만) 찾기(`report::empty`, 10분 안에 만든 폴더·제외·보호·링크·숨김 제외), 홈 "빈 폴더 N개 [살펴보기] [정리]"(체크 목록), `Mover::remove_empty_dirs`(직전 재확인, 잡파일은 휴지통, 폴더는 `remove_dir`, move_log `op = rmdir`, 되돌리기 = 폴더 다시 만들기), 그대로 둔 폴더 이유 "빈 폴더" 구분, `tidy empty`. SPEC §14.20. 창 확인은 로컬에서
- [x] M19 정리 위치 점검 (2026-11-12, 0.15.0) — 정리 위치 안을 통째로 보고 재정렬하는 계획 하나(`report::review`, `PlanSource::Review`): 철자만 다른 같은 폴더·`기타` 아래 고객사 폴더 합치기, 같은 이슈 `merge_days` 안 폴더 합치기(폴더 통째, 코드 프로젝트 제외), 작은 폴더 접기(compact), 맨 위 파일 규칙, "AI로 이것도 보기"(고객사 이름 보이는 파일·맨 위 파일 → `Curator::place_files`, 실패해도 나머지 유지), 정리하기 "정리 위치 점검 — D:\정리" 선택지·홈 [정리 위치 점검], `tidy review`, e2e m19. SPEC §14.21. 실제 D:\정리에서의 결과·창 확인은 로컬에서
- [x] 0.16.0 정리 위치 새로 짜기 + 고객사 자료는 고객사 폴더로 (2026-11-12) — `report::rebuild`(`PlanSource::Rebuild`, 오프라인): 점검의 합치기 + 이름·본문(인덱스 앞 20,000자, 고객사 하나만)에 고객사 보이는 파일은 어디 있든 `{고객사}/{종류}`(`curate::kinds`), 고객사 이름 폴더는 통째로, 나머지는 종류·연도 폴더에서만 규칙, 최대 20,000개, 정리하기 [처음부터 새로 짜기 미리보기]·`tidy review --rebuild`. 감시 중 고객사 파일은 AI 실패·없음이면 규칙 대신 `{고객사}/{종류}`(`Engine::customer_kind_step`, 새 고객사는 확인 대기), 점검도 고객사 파일 오프라인 배치. 미리보기 diff 오른쪽 칸 넘침 수정, 미리보기 300개씩 더 보기. SPEC §14.22. 실제 D:\정리 결과·창 확인은 로컬에서
- [x] 0.16.1 수정 (2026-11-12) — 새로 짜기가 `텍스트/2026` 파일에 연·월 규칙을 다시 적용해 `텍스트/2026/06`·`1980/01`로 맴돌던 문제(규칙은 맨 위 폴더가 다를 때만), 정리 위치 목록 내보내기(`report::listing`, JSON: 경로·크기·날짜·원래 폴더·본문 앞 160자, 다운로드 폴더)→Claude가 계획 파일→[받은 계획 파일 열기], `tidy review --export`, e2e m19 F. SPEC §14.22
- [x] 0.2.x 핫픽스 (2026-10-22~23) — 규칙 카드 정렬(#12), AI 응답 잘림 분할 재시도(#13), HTTP 타임아웃 상한(#14), 감시 폴더 제외 폴더(SPEC §14.5)

## 열린 결정
SPEC §13 참조. 결정되면 여기와 SPEC 양쪽 갱신.
- 한국어 FTS 토크나이저 → **결정(M2)**: trigram 주 + unicode61 보조(1~2글자 prefix). `docs/fts-tokenizer.md`.
- sqlite-vec vs brute-force → **결정(M7)**: BLOB + 메모리 int8 brute-force(10만×768 질의 ~25ms). 50만 파일 넘으면 재검토.
- Linux 키 저장소 → **결정(M7)**: Secret Service, 없으면 keyutils 폴백.
- `.hwp` 본문 추출 → **결정(M14)**: `cfb` + raw deflate + 레코드 파서 자체 구현(HWPTAG_PARA_TEXT), `.hwpx`는 ZIP+XML. SPEC §13·§14.9.
