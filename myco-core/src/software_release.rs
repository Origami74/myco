//! Authenticate Myco's Zapstore announcements with the publisher in zapstore.yaml.
use nostr::{Event, PublicKey};

const PUBLISHER: &str = "npub1hw6amg8p24ne08c9gdq8hhpqx0t0pwanpae9z25crn7m9uy7yarse465gr";

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn verify(json: &str) -> bool {
    let Ok(event) = serde_json::from_str::<Event>(json) else {
        return false;
    };
    let Ok(publisher) = PublicKey::parse(PUBLISHER) else {
        return false;
    };
    event.pubkey == publisher && event.kind.as_u16() == 30063 && event.verify().is_ok()
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
}
