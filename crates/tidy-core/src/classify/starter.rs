//! Starter rules offered by the first-run wizard (SPEC §14.1). Nothing
//! here is ever applied on its own: only the ones the user ticks are
//! written to `rules.toml`. All destinations are relative, so they stay
//! inside the folder (`in_place`) or under `organize_root` (`collect`).

use serde::Serialize;

use super::rules_doc::RuleCard;

#[derive(Debug, Clone, Serialize)]
pub struct Starter {
    pub id: &'static str,
    pub title: &'static str,
    /// One line: what goes where.
    pub summary: &'static str,
    pub card: RuleCard,
}

fn card(name: &str, ext: &[&str], name_regex: &str, dest: &str, confirm: bool) -> RuleCard {
    RuleCard {
        name: name.into(),
        ext: ext.iter().map(|e| e.to_string()).collect(),
        name_regex: name_regex.into(),
        dest: dest.into(),
        confirm,
        ..Default::default()
    }
}

/// In priority order (a screenshot is also an image: it comes first).
pub fn starters() -> Vec<Starter> {
    vec![
        Starter {
            id: "screenshots",
            title: "스크린샷",
            summary: "화면 캡처 이미지 → 캡처/연도-월",
            card: card(
                "스크린샷",
                &["png", "jpg", "jpeg"],
                "^(스크린샷|Screenshot|Screen Shot|화면 캡처)",
                "캡처/{yyyy}-{mm}",
                false,
            ),
        },
        Starter {
            id: "installers",
            title: "설치파일",
            summary: "exe·msi·dmg·pkg → 설치파일 (옮기기 전에 확인)",
            card: card(
                "설치파일",
                &["exe", "msi", "dmg", "pkg"],
                "",
                "설치파일",
                true,
            ),
        },
        Starter {
            id: "documents",
            title: "문서",
            summary: "pdf·한글·워드·엑셀·파워포인트 → 문서/연도-월",
            card: card(
                "문서",
                &[
                    "pdf", "hwp", "hwpx", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
                ],
                "",
                "문서/{yyyy}-{mm}",
                false,
            ),
        },
        Starter {
            id: "archives",
            title: "압축 파일",
            summary: "zip·7z·rar·alz·egg → 압축 파일",
            card: card(
                "압축 파일",
                &["zip", "7z", "rar", "alz", "egg"],
                "",
                "압축 파일",
                false,
            ),
        },
        Starter {
            id: "media",
            title: "사진·영상",
            summary: "사진과 동영상 → 사진·영상/연도-월",
            card: card(
                "사진·영상",
                &[
                    "jpg", "jpeg", "png", "heic", "gif", "webp", "mp4", "mov", "avi", "mkv",
                ],
                "",
                "사진·영상/{yyyy}-{mm}",
                false,
            ),
        },
    ]
}

/// Cards for the ticked ids, in the list's priority order.
pub fn starter_cards(ids: &[String]) -> Vec<RuleCard> {
    starters()
        .into_iter()
        .filter(|s| ids.iter().any(|i| i == s.id))
        .map(|s| s.card)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::rules_doc::apply_cards;
    use crate::classify::RuleSet;

    #[test]
    fn starters_are_valid_relative_rules() {
        let all: Vec<String> = starters().iter().map(|s| s.id.to_string()).collect();
        let text = apply_cards("", &starter_cards(&all)).unwrap();
        let rules = RuleSet::from_toml(&text).unwrap();
        assert_eq!(rules.rules().len(), 5);
        assert!(rules.dest_roots().is_empty(), "no absolute destinations");
        // Only ticked ones, in priority order whatever the tick order.
        let some = starter_cards(&["media".into(), "screenshots".into()]);
        assert_eq!(some[0].name, "스크린샷");
        assert_eq!(some.len(), 2);
    }
}
