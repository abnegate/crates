use tokio::sync::mpsc;

use crate::error::ExecutorError;

/// Handle to a running job's stdin
#[derive(Debug)]
pub struct StdinHandle {
    pub(super) tx: mpsc::Sender<Vec<u8>>,
}

impl StdinHandle {
    /// Send data to the process's stdin
    pub async fn send(&self, data: Vec<u8>) -> Result<(), ExecutorError> {
        self.tx
            .send(data)
            .await
            .map_err(|_| ExecutorError::ChannelClosed)
    }

    /// Close the stdin (signals EOF to the process)
    pub fn close(self) {
        drop(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stdin_handle_send() {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(10);
        let stdin_handle = StdinHandle { tx };

        let result = stdin_handle.send(b"hello".to_vec()).await;
        assert!(result.is_ok());

        let received = rx.recv().await.unwrap();
        assert_eq!(received, b"hello".to_vec());
    }

    #[tokio::test]
    async fn test_stdin_handle_send_closed_channel() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(10);
        let stdin_handle = StdinHandle { tx };

        drop(rx);

        let result = stdin_handle.send(b"hello".to_vec()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ExecutorError::ChannelClosed => {}
            e => panic!("Expected ChannelClosed, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_stdin_handle_close() {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(10);
        let stdin_handle = StdinHandle { tx };

        stdin_handle.close();

        assert!(rx.recv().await.is_none());
    }
}
