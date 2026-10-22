//! Query and authenticate Myco's publisher-signed Zapstore announcements.
use nostr::{Event, PublicKey};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PUBLISHER: &str = "npub1hw6amg8p24ne08c9gdq8hhpqx0t0pwanpae9z25crn7m9uy7yarse465gr";
const RELAY: &str = "wss://relay.zapstore.dev";

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn verify(json: &str) -> bool {
    serde_json::from_str::<Event>(json).is_ok_and(|event| authenticated(&event))
}

fn authenticated(event: &Event) -> bool {
    PublicKey::parse(PUBLISHER).is_ok_and(|publisher| {
        event.pubkey == publisher && event.kind.as_u16() == 30063 && event.verify().is_ok()
    })
}

// Match Android's stable version policy, including optional v and build metadata.
fn stable_version(tag: &str) -> Option<[u64; 3]> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let mut parts = tag.split('+');
    let version = parts.next()?;
    if let Some(build) = parts.next() {
        if parts.next().is_some()
            || build
                .split('.')
                .any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
        {
            return None;
        }
    }
    let numbers: Vec<_> = version.split('.').collect();
    if numbers.len() != 3 {
        return None;
    }
    let mut result = [0; 3];
    for (out, part) in result.iter_mut().zip(numbers) {
        if part.is_empty()
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        *out = part.parse().ok()?;
        if *out > i64::MAX as u64 {
            return None;
        }
    }
    Some(result)
}

fn announcement_tag(event: &Event, now: u64) -> Option<String> {
    if event.created_at.as_secs() > now.saturating_add(300) {
        return None;
    }
    let values = |name: &str| -> Vec<&str> {
        event
            .tags
            .iter()
            .filter_map(|tag| {
                let fields = tag.as_slice();
                (fields.first().map(String::as_str) == Some(name))
                    .then(|| fields.get(1).map(String::as_str))
                    .flatten()
            })
            .collect()
    };
    if values("i") != ["app.myco"] {
        return None;
    }
    let versions = values("version");
    if versions.len() != 1 {
        return None;
    }
    let version = versions[0];
    stable_version(version)?;
    if values("d") != [format!("app.myco@{version}").as_str()] {
        return None;
    }
    if !values("e").iter().any(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }) {
        return None;
    }
    let channels = values("c");
    if !channels.is_empty() && channels != ["main"] {
        return None;
    }
    Some(version.to_owned())
}

fn newest(events: impl IntoIterator<Item = Event>, now: u64) -> anyhow::Result<String> {
    events
        .into_iter()
        .filter(|event| authenticated(event))
        .filter_map(|event| announcement_tag(&event, now))
        .max_by_key(|tag| stable_version(tag).unwrap())
        .ok_or_else(|| anyhow::anyhow!("No verified Myco release found"))
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub async fn fetch() -> anyhow::Result<String> {
    let publisher = PublicKey::parse(PUBLISHER)?;
    let filter = serde_json::json!({
        "kinds": [30063], "authors": [publisher.to_hex()],
        "#i": ["app.myco"], "limit": 100,
    });
    let events = tokio::time::timeout(
        Duration::from_secs(20),
        crate::ip_source::query_relay(RELAY, filter),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Release query timed out"))??;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    newest(events, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    #[test]
    fn rejects_other_publishers_even_with_valid_signatures() {
        let event = EventBuilder::new(Kind::Custom(30063), "")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        assert!(event.verify().is_ok());
        assert!(!verify(&serde_json::to_string(&event).unwrap()));
    }

    #[test]
    fn authentic_release_accepts_but_tampering_rejects() {
        let json = include_str!("software-release-fixture.json");
        assert!(verify(json));
        let mut event: serde_json::Value = serde_json::from_str(json).unwrap();
        event["content"] = serde_json::json!("tampered");
        assert!(!verify(&event.to_string()));
        event["pubkey"] = serde_json::json!(Keys::generate().public_key().to_hex());
        assert!(!verify(&event.to_string()));
    }

    #[test]
    fn rejects_malformed_events() {
        assert!(!verify("{}"));
        assert!(!verify("not json"));
    }

    fn metadata(version: &str, app: &str, channel: &str) -> Event {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("software-release-fixture.json")).unwrap();
        value["created_at"] = serde_json::json!(100);
        value["tags"] = serde_json::json!([
            ["i", app],
            ["d", format!("{app}@{version}")],
            ["version", version],
            ["c", channel],
            ["e", "a".repeat(64)]
        ]);
        // Metadata tests bypass authentication deliberately; separate tests verify signatures.
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn validates_announcement_fields() {
        assert_eq!(
            announcement_tag(&metadata("0.12.0", "app.myco", "main"), 100).as_deref(),
            Some("0.12.0")
        );
        for event in [
            metadata("0.12.0", "other.app", "main"),
            metadata("0.12.0", "app.myco", "beta"),
            metadata("0.12.0-rc.1", "app.myco", "main"),
        ] {
            assert!(announcement_tag(&event, 100).is_none());
        }
        let original = metadata("0.12.0", "app.myco", "main");
        for (field, replacement) in [
            ("created_at", serde_json::json!(1000)),
            (
                "tags",
                serde_json::json!([
                    ["i", "app.myco"],
                    ["d", "app.myco@0.12.0"],
                    ["version", "0.12.0"]
                ]),
            ),
            (
                "tags",
                serde_json::json!([
                    ["i", "app.myco"],
                    ["i", "app.myco"],
                    ["d", "app.myco@0.12.0"],
                    ["version", "0.12.0"],
                    ["e", "a".repeat(64)]
                ]),
            ),
            (
                "tags",
                serde_json::json!([
                    ["i", "app.myco"],
                    ["d", "app.myco@0.13.0"],
                    ["version", "0.12.0"],
                    ["e", "a".repeat(64)]
                ]),
            ),
        ] {
            let mut value = serde_json::to_value(&original).unwrap();
            value[field] = replacement;
            assert!(announcement_tag(&serde_json::from_value(value).unwrap(), 100).is_none());
        }
    }

    #[test]
    fn orders_stable_versions_numerically() {
        assert!(stable_version("0.12.0") > stable_version("0.9.9"));
        assert_eq!(stable_version("v0.12.0+build.1"), Some([0, 12, 0]));
        for tag in [
            "0.12.0-rc.1",
            "01.2.3",
            "1.2",
            "1.2.3+",
            "1.2.3+a+b",
            "9223372036854775808.0.0",
        ] {
            assert!(stable_version(tag).is_none(), "{tag}");
        }
    }

    #[test]
    fn selects_only_authenticated_announcements() {
        let real: Event =
            serde_json::from_str(include_str!("software-release-fixture.json")).unwrap();
        let now = real.created_at.as_secs();
        assert_eq!(
            newest(vec![metadata("99.0.0", "app.myco", "main"), real], now).unwrap(),
            "0.11.0"
        );
        assert!(newest(vec![metadata("99.0.0", "app.myco", "main")], now).is_err());
    }
}
