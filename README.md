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
