//! NAP-THEME — the colours a napplet should draw in.
//!
//! ```text
//! -> { "type": "theme.get", "id": "t1" }
//! <- { "type": "theme.get.result", "id": "t1",
//!      "theme": { "colors": { "background": "#ffffff", "text": "#0f172a", "primary": "#059669" },
//!                 "title": "Myco Light" } }
//! <- { "type": "theme.changed", "theme": { … } }            (pushed)
//! ```
//!
//! Read-only and harmless: a napplet learns whether the phone is in light or
//! dark mode, which its own `prefers-color-scheme` already tells it. There is
//! no picker yet — the two themes mirror Myco's own palette
//! (`android/…/ui/theme/Theme.kt`), and which one applies follows the app's
//! light/dark mode, which the window host reports per session
//! ([`Session::set_appearance`]). A session nobody told defaults to light.

use crate::seams::Envelope;
use crate::session::{Appearance, Session};

/// The theme for an appearance, as NAP-THEME's `theme` object.
pub fn theme_for(appearance: Appearance) -> serde_json::Value {
    match appearance {
        Appearance::Light => serde_json::json!({
            "colors": { "background": "#ffffff", "text": "#0f172a", "primary": "#059669" },
            "title": "Myco Light",
        }),
        Appearance::Dark => serde_json::json!({
            "colors": { "background": "#000000", "text": "#ffffff", "primary": "#34d399" },
            "title": "Myco AMOLED",
        }),
    }
}

/// Handle an inbound `theme.*` message.
pub fn handle(session: &Session, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "get" => vec![message
            .to_result()
            .with_field("theme", theme_for(session.appearance()))],
        // `theme.changed` is runtime -> napplet only; anything else is unknown.
        _ => Vec::new(),
    }
}

/// The push a session receives when its appearance changes — `None` when it
/// should hear nothing: not yet handshaken, or not granted `theme`.
pub fn changed_frame(session: &Session) -> Option<Envelope> {
    session.may_service("theme").then(|| {
        Envelope::new("theme.changed").with_field("theme", theme_for(session.appearance()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::NappletIdentity;
    use crate::testing::test_context;
    use serde_json::json;

    fn session(granted: &[&str]) -> Session {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), granted.to_vec());
        s.on_ready();
        s
    }

    #[tokio::test]
    async fn theme_get_answers_light_by_default() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["theme"]);
        let out = dispatch(&ctx, &mut s, &Envelope::new("theme.get").with_id("t1")).await;
        assert_eq!(
            serde_json::to_value(&out.envelopes()[0]).unwrap(),
            json!({
                "type": "theme.get.result",
                "id": "t1",
                "theme": {
                    "colors": {"background": "#ffffff", "text": "#0f172a", "primary": "#059669"},
                    "title": "Myco Light"
                }
            })
        );
    }

    #[tokio::test]
    async fn theme_get_follows_the_reported_appearance() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["theme"]);
        assert!(s.set_appearance(Appearance::Dark));
        let out = dispatch(&ctx, &mut s, &Envelope::new("theme.get").with_id("t1")).await;
        let theme = &out.envelopes()[0].field("theme").unwrap().clone();
        assert_eq!(theme["title"], "Myco AMOLED");
        assert_eq!(theme["colors"]["background"], "#000000");
        assert_eq!(theme["colors"]["text"], "#ffffff");
        assert_eq!(theme["colors"]["primary"], "#34d399");
    }

    /// `theme.get` reads the session and changes nothing, so a host may run it
    /// against a snapshot.
    #[test]
    fn theme_get_is_stateless() {
        assert!(!crate::dispatch::needs_session(&Envelope::new("theme.get")));
    }

    #[test]
    fn a_change_is_pushed_only_to_a_granted_established_session() {
        let mut s = session(&["theme"]);
        s.set_appearance(Appearance::Dark);
        let push = changed_frame(&s).unwrap();
        assert_eq!(push.msg_type, "theme.changed");
        assert!(push.id.is_none());
        assert_eq!(push.field("theme").unwrap()["title"], "Myco AMOLED");

        assert!(changed_frame(&session(&[])).is_none(), "ungranted");
        let fresh = Session::new(NappletIdentity::new("chat", "aggregate"), ["theme"]);
        assert!(changed_frame(&fresh).is_none(), "not handshaken");
    }

    #[test]
    fn reporting_the_same_appearance_is_not_a_change() {
        let mut s = session(&["theme"]);
        assert!(!s.set_appearance(Appearance::Light));
        assert!(s.set_appearance(Appearance::Dark));
        assert!(!s.set_appearance(Appearance::Dark));
    }

    #[tokio::test]
    async fn an_ungranted_theme_get_is_refused() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["relay"]);
        let out = dispatch(&ctx, &mut s, &Envelope::new("theme.get").with_id("t1")).await;
        assert!(out.envelopes()[0].field("error").is_some());
    }
}
