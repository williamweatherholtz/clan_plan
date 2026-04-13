use lettre::{
    message::header::ContentType, transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

use crate::config::Config;

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl Mailer {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let transport = if config.smtp_tls {
            let creds = Credentials::new(
                config.smtp_user.clone(),
                config.smtp_password.clone(),
            );
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)?
                .port(config.smtp_port)
                .credentials(creds)
                .build()
        } else {
            // For local dev (Mailpit) — no TLS, no auth
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
                .port(config.smtp_port)
                .build()
        };

        Ok(Self {
            transport,
            from: config.smtp_from.clone(),
        })
    }

    pub async fn send(&self, to: &str, subject: &str, body: &str) -> anyhow::Result<()> {
        let message = Message::builder()
            .from(self.from.parse()?)
            .to(to.parse()?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())?;

        self.transport.send(message).await?;
        Ok(())
    }

    pub async fn send_verification_email(
        &self,
        to_email: &str,
        display_name: &str,
        verify_url: &str,
    ) -> anyhow::Result<()> {
        let body = format!(
            "Hi {display_name},\n\n\
             Please verify your email address to activate your Clan Plan account:\n\n\
             {verify_url}\n\n\
             This link expires in 24 hours.\n\n\
             If you did not create an account, you can safely ignore this email."
        );
        self.send(to_email, "Verify your Clan Plan account", &body).await
    }

    pub async fn send_password_reset_email(
        &self,
        to_email: &str,
        display_name: &str,
        reset_url: &str,
    ) -> anyhow::Result<()> {
        let body = format!(
            "Hi {display_name},\n\n\
             A password reset was requested for your Clan Plan account.\n\n\
             Click the link below to set a new password:\n\n\
             {reset_url}\n\n\
             This link expires in 1 hour. If you did not request a reset, \
             you can safely ignore this email — your password has not changed."
        );
        self.send(to_email, "Reset your Clan Plan password", &body).await
    }

    pub async fn send_announcement_email(
        &self,
        to_email: &str,
        display_name: &str,
        reunion_title: &str,
        announcement_title: &str,
        announcement_content: &str,
        app_url: &str,
    ) -> anyhow::Result<()> {
        let body = format!(
            "Hi {display_name},\n\n\
             New announcement for \"{reunion_title}\":\n\n\
             {announcement_title}\n\
             {}\n\n\
             {announcement_content}\n\n\
             Visit Clan Plan:\n{app_url}\n",
            "─".repeat(announcement_title.len())
        );
        self.send(
            to_email,
            &format!("[{reunion_title}] {announcement_title}"),
            &body,
        )
        .await
    }

    pub async fn send_phase_notification(
        &self,
        to_email: &str,
        display_name: &str,
        reunion_title: &str,
        phase_label: &str,
        app_url: &str,
    ) -> anyhow::Result<()> {
        let body = format!(
            "Hi {display_name},\n\n\
             The reunion \"{reunion_title}\" has moved to a new phase: {phase_label}.\n\n\
             Visit Clan Plan to see what's next:\n\n\
             {app_url}\n"
        );
        self.send(
            to_email,
            &format!("{reunion_title} — now in {phase_label}"),
            &body,
        )
        .await
    }
}
