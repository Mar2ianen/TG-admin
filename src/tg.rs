#[derive(Debug, Clone)]
pub struct TelegramGateway {
    polling: bool,
}

impl TelegramGateway {
    pub fn new(polling: bool) -> Self {
        Self { polling }
    }

    pub fn polling(&self) -> bool {
        self.polling
    }
}
