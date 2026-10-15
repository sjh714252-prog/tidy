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
cargo run -p tidy-cli -- search "키워드"
cd apps/desktop && npm run tauri dev
```

## 진행 상태
- [x] M0 워크스페이스 + `tidy-cli scan` (2026-10-15)
- [x] M1 Watcher + Rules + Mover + Undo (2026-10-15)
- [x] M2 Store + Indexer + FTS 검색 (2026-10-15) — 토크나이저 결정: docs/fts-tokenizer.md
- [ ] M3 Tauri 앱 기본 플로우
- [ ] M4 AI Layer + 학습 루프
- [ ] M5 Reporter + 재구성 diff
- [ ] M6 NAS 폴링/복사-검증 이동
- [ ] M7 의미검색 + 패키징

## 열린 결정
SPEC §13 참조. 결정되면 여기와 SPEC 양쪽 갱신.
- 한국어 FTS 토크나이저 → **결정(M2)**: trigram 주 + unicode61 보조(1~2글자 prefix). `docs/fts-tokenizer.md`.
