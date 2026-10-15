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
