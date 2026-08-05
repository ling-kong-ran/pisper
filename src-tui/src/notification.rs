use crate::app::App;

const MAX_NOTIFICATION_SUMMARY_CHARS: usize = 320;

#[derive(Debug, Eq, PartialEq)]
pub struct ChatCompletion {
    pub title: String,
    pub summary: String,
    pub model: String,
}

pub fn chat_completion(app: &App) -> ChatCompletion {
    let title = match app.session.name.trim() {
        "" | "New conversation" => "Pisper conversation",
        name => name,
    };
    let response = app
        .live
        .as_ref()
        .map(|live| live.text_target.trim())
        .filter(|text| !text.is_empty())
        .unwrap_or("The Agent has finished responding.");
    let summary = response.split_whitespace().collect::<Vec<_>>().join(" ");

    ChatCompletion {
        title: title.to_owned(),
        summary: summary
            .chars()
            .take(MAX_NOTIFICATION_SUMMARY_CHARS)
            .collect(),
        model: app.model.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::chat_completion;
    use crate::{
        app::{App, LiveTurn},
        model::SessionSummary,
    };

    #[test]
    fn completion_notifications_use_the_session_title_and_final_response() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Resize audit".to_owned(),
            model: "provider/model".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        );
        app.live = Some(LiveTurn {
            text_target: "The resize fix is complete.\nAll tests passed.".to_owned(),
            ..LiveTurn::default()
        });

        assert_eq!(
            chat_completion(&app),
            super::ChatCompletion {
                title: "Resize audit".to_owned(),
                summary: "The resize fix is complete. All tests passed.".to_owned(),
                model: "provider/model".to_owned(),
            }
        );
    }
}
