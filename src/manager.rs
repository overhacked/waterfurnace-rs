use crate::Session;
use crate::client::Result as SessionResult;
use crate::client::state;


pub struct SessionManager {
    session: Option<Session<state::Connected>>,
    username: String,
    password: String,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("session", &self.session)
            .field("username", &self.username)
            .field("password", &"********")
            .finish()
    }
}

impl SessionManager {
    pub async fn new(username: &str, password: &str)
        -> SessionResult<SessionManager>
    {
        Ok(SessionManager {
            session: None,
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    pub async fn login(&mut self)
        -> SessionResult<()>
    {
        // Idempotent no-op if already logged in
        if self.session.is_none() {
            self.session = Some(
                Session::new()
                .login(&self.username, &self.password).await?
                .connect().await?
            );
        }

        Ok(())
    }
            
    pub async fn logout(&mut self)
        -> SessionResult<()>
    {
        // Idempotent no-op if already logged out
        if self.session.is_some() {
            let connected_session = self.session.take().unwrap();
            connected_session.logout().await?;
        }

        Ok(())
    }
}
