# 패키징·배포 (M7)

Tidy 데스크톱 앱은 `tauri build`로 OS별 설치 파일을 만든다.

| OS | 산출물 | 위치 (`apps/desktop/src-tauri/target/…`) |
|---|---|---|
| Windows | `.msi` (WiX, ko-KR), `.exe` (NSIS, 사용자 단위 설치) | `release/bundle/msi/`, `release/bundle/nsis/` |
| macOS | `.app`, `.dmg` (universal: Apple Silicon + Intel) | `universal-apple-darwin/release/bundle/dmg/` |
| Linux | `.deb`, `.AppImage` | `release/bundle/deb/`, `release/bundle/appimage/` |

## GitHub Actions (`.github/workflows/release.yml`)

- `v*` 태그를 푸시하면 세 OS에서 설치 파일을 만들어 **초안(draft) 릴리스**에 붙인다.
  확인 후 GitHub에서 직접 게시한다.
  ```
  git tag v0.1.0 && git push origin v0.1.0
  ```
- Actions 탭 → Release → *Run workflow*로 수동 실행하면 릴리스 없이 워크플로 아티팩트로만 받는다.
- 각 OS 빌드 뒤에 **릴리스 바이너리에 개발용 테스트 브리지가 없는지** 검사한다
  (`dev bridge`, `tidy-devbridge`, `127.0.0.1:1421` 문자열이 있으면 실패).
- 서명: 현재 코드 서명/공증은 하지 않는다. Windows SmartScreen은 "추가 정보 → 실행",
  macOS는 처음 한 번 Finder에서 우클릭 → 열기가 필요하다. 서명 인증서가 생기면
  `TAURI_SIGNING_*`/`APPLE_*` 시크릿을 워크플로에 추가한다.

## 로컬 빌드

공통: Rust stable, Node LTS.

```bash
cd apps/desktop
npm ci
npx tauri build                          # 이 OS의 기본 번들 전부
npx tauri build --bundles msi,nsis       # Windows
npx tauri build --bundles app,dmg --target universal-apple-darwin   # macOS (두 타깃 rustup target add 필요)
npx tauri build --bundles deb,appimage   # Linux
```

OS별 준비물:

- **Windows**: Visual Studio Build Tools(“C++를 사용한 데스크톱 개발”), WebView2 런타임(Windows 10/11 기본 포함).
  WiX/NSIS 도구는 tauri-cli가 처음 빌드할 때 내려받는다.
- **macOS**: Xcode Command Line Tools. universal 빌드는
  `rustup target add aarch64-apple-darwin x86_64-apple-darwin`.
- **Linux (Ubuntu/Debian)**:
  ```bash
  sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
    libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev libxdo-dev patchelf file
  ```
  AppImage는 linuxdeploy를 처음 한 번 내려받는다(네트워크 필요).

### 개발용 테스트 브리지는 릴리스에 없다

`src-tauri/src/devbridge.rs`(127.0.0.1:1421 HTTP)는 `#[cfg(debug_assertions)]`로만 컴파일되고,
프론트엔드의 브리지 코드(`src/ipc/transport.ts`)는 `import.meta.env.DEV`로 감싸져 있어 `vite build`에서
제거된다. `tauri build`(release 프로필)에는 둘 다 들어가지 않는다. 직접 확인:

```bash
grep -a -c -e "dev bridge" -e "tidy-devbridge" -e "127.0.0.1:1421" src-tauri/target/release/tidy-desktop   # 0이어야 함
```

`tauri build --debug`는 디버그 빌드이므로 브리지가 **들어간다** — 배포용으로 쓰지 않는다.

## 자동 시작 (로그인 시 트레이 상주)

설정 → 일반 → “로그인 시 자동 시작”(`[general] start_on_login`, 기본 켜짐).
릴리스 빌드는 앱이 시작될 때와 설정을 저장할 때 OS 시작 항목을 이 값에 맞춘다.

| OS | 등록 위치 |
|---|---|
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 의 `Tidy` 값 |
| macOS | `~/Library/LaunchAgents/Tidy.plist` (LaunchAgent) |
| Linux | `~/.config/autostart/Tidy.desktop` (AppImage면 AppImage 경로) |

등록된 명령에는 `--autostart`가 붙고, 이 인자로 시작하면 창을 띄우지 않고 트레이에만 상주한다.
개발 빌드(`tauri dev`)는 `target/debug` 경로가 등록되는 것을 막기 위해 등록하지 않는다
(테스트할 때만 `TIDY_DEV_AUTOSTART=1`).

## 제거

- Windows: 설정 → 앱 → Tidy → 제거 (MSI/NSIS 모두). 자동 시작 레지스트리 값은 남을 수 있으니,
  제거 **전에** 설정에서 자동 시작을 끄거나 제거 후 `시작 프로그램` 목록에서 확인한다.
- macOS: `/Applications/Tidy.app` 삭제 + 필요하면 `~/Library/LaunchAgents/Tidy.plist` 삭제.
- Linux: `sudo apt remove tidy` (deb) / AppImage 파일 삭제 + `~/.config/autostart/Tidy.desktop` 삭제.

설정(`config.toml`)과 DB(`tidy.db`)는 사용자 데이터라 제거해도 남는다
(Windows `%APPDATA%\tidy\`·`%LOCALAPPDATA%\tidy\`, macOS `~/Library/Application Support/tidy/`, Linux `~/.config/tidy/`·`~/.local/share/tidy/`).
API 키는 OS 키체인(Windows 자격 증명 관리자 / macOS 키체인 / Linux Secret Service)에 `tidy/…` 이름으로 남으므로
필요하면 설정 → AI에서 “키 삭제” 후 제거한다.
