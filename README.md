# Tidy

바탕화면·지정 폴더를 상시 감시해 규칙/AI 판단으로 정리하고, 본문까지 인덱싱해 검색하는 크로스플랫폼 데스크톱 앱.

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
