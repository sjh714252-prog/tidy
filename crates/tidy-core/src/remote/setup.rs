//! (M18) The `docker-compose.yml` the PC app hands the user for their NAS
//! ("설치 파일 만들기"): the agent image, the password the PC keeps in its
//! keychain, and one `/data/<share>` mount per shared folder.

use super::api::DEFAULT_PORT;

/// Image the compose file pulls.
pub const IMAGE: &str = "ghcr.io/sjh714252-prog/tidy-agent:latest";

/// Share names as typed ("업무, 자료" or one per line), cleaned: no slashes,
/// no duplicates, in order.
pub fn parse_shares(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in input.split([',', '\n', ';']) {
        let s = s.trim().trim_matches('/').trim();
        let s = s.rsplit('/').next().unwrap_or(s).trim();
        if !s.is_empty() && !s.contains(['\\', ':', '"']) && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    out
}

/// YAML double-quoted string.
fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `volume` is the NAS volume the shares live on (Synology: `/volume1`).
pub fn compose_file(token: &str, shares: &[String], volume: &str) -> String {
    let volume = volume.trim().trim_end_matches('/');
    let volume = if volume.is_empty() {
        "/volume1"
    } else {
        volume
    };
    let mut mounts = String::new();
    if shares.is_empty() {
        mounts.push_str(&format!(
            "      # - {volume}/공유폴더이름:/data/공유폴더이름   ← 정리할 공유 폴더마다 한 줄\n"
        ));
    }
    for s in shares {
        mounts.push_str(&format!(
            "      - {}\n",
            q(&format!("{volume}/{s}:/data/{s}"))
        ));
    }
    format!(
        "# Tidy NAS 에이전트 — Synology Container Manager › 프로젝트 › 생성에서\n\
         # 경로를 {volume}/docker/tidy 로 고르고 이 내용을 붙여 넣으세요.\n\
         # (QNAP Container Station › 애플리케이션 › 생성, 또는 docker compose up -d)\n\
         # TIDY_TOKEN은 PC의 Tidy와 같은 연결 암호입니다. 다른 사람에게 보이지 마세요.\n\
         services:\n\
         \x20 tidy-agent:\n\
         \x20   image: {IMAGE}\n\
         \x20   container_name: tidy-agent\n\
         \x20   restart: unless-stopped\n\
         \x20   ports:\n\
         \x20     - \"{DEFAULT_PORT}:{DEFAULT_PORT}\"\n\
         \x20   environment:\n\
         \x20     TIDY_TOKEN: {token}\n\
         \x20     TZ: Asia/Seoul\n\
         \x20   # 새로 만드는 폴더의 소유자를 NAS 사용자로 하려면 주석을 풀고 uid:gid를 넣으세요.\n\
         \x20   # user: \"1026:100\"\n\
         \x20   volumes:\n\
         \x20     - {config}\n\
         {mounts}",
        token = q(token),
        config = q(&format!("{volume}/docker/tidy:/config")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_are_cleaned() {
        assert_eq!(
            parse_shares(" 업무, /volume1/자료/ \n업무;; 사진 "),
            ["업무", "자료", "사진"]
        );
        assert!(parse_shares(" , ").is_empty());
        assert!(parse_shares("a\"b").is_empty());
    }

    #[test]
    fn compose_has_token_and_mounts() {
        let y = compose_file("abc123", &["업무".into(), "사진 모음".into()], "/volume2/");
        assert!(y.contains("TIDY_TOKEN: \"abc123\""));
        assert!(y.contains("- \"/volume2/업무:/data/업무\""));
        assert!(y.contains("- \"/volume2/사진 모음:/data/사진 모음\""));
        assert!(y.contains("- \"/volume2/docker/tidy:/config\""));
        assert!(y.contains("\"7841:7841\""));
        assert!(y.contains(IMAGE));
        // Indentation is YAML: two spaces per level.
        assert!(y.contains("\n  tidy-agent:\n    image: "));
        let empty = compose_file("t", &[], "");
        assert!(empty.contains("# - /volume1/공유폴더이름"));
    }
}
