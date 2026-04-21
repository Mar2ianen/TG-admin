use anyhow::Result;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ShutdownReason {
    SignalInt,
    SignalTerm,
    #[cfg(test)]
    Immediate,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ShutdownController {
    OsSignals,
    #[cfg(test)]
    Immediate,
}

impl ShutdownController {
    pub const fn os_signals() -> Self {
        Self::OsSignals
    }

    #[cfg(test)]
    pub const fn immediate() -> Self {
        Self::Immediate
    }

    pub async fn wait(self) -> Result<ShutdownReason> {
        match self {
            Self::OsSignals => wait_for_os_signal().await,
            #[cfg(test)]
            Self::Immediate => Ok(ShutdownReason::Immediate),
        }
    }
}

#[cfg(unix)]
async fn wait_for_os_signal() -> Result<ShutdownReason> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result?;
            Ok(ShutdownReason::SignalInt)
        }
        _ = terminate.recv() => Ok(ShutdownReason::SignalTerm),
    }
}

#[cfg(not(unix))]
async fn wait_for_os_signal() -> Result<ShutdownReason> {
    tokio::signal::ctrl_c().await?;
    Ok(ShutdownReason::SignalInt)
}
