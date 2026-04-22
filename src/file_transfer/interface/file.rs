use std::path::PathBuf;

use tracing::instrument;

const INTERFACE: &str = "io.edgehog.devicemanager.storage.File";

#[derive(Debug, Clone)]
pub(crate) struct StoredFile<S = String> {
    id: S,
    path: PathBuf,
    size: i64,
}

impl<S> StoredFile<S> {
    pub(crate) fn create(id: S, path: PathBuf, size: u64) -> Self {
        Self {
            id,
            path,
            size: super::to_i64(size),
        }
    }

    #[instrument(skip_all)]
    pub(crate) async fn send<C>(self, device: &mut C) -> eyre::Result<()>
    where
        C: astarte_device_sdk::Client + Send + Sync + 'static,
        S: std::fmt::Display,
    {
        let path = self.path.display().to_string();

        device
            .set_property(
                INTERFACE,
                &format!("/{}/pathOnDevice", self.id),
                path.into(),
            )
            .await?;

        device
            .set_property(
                INTERFACE,
                &format!("/{}/sizeBytes", self.id),
                self.size.into(),
            )
            .await
            .map_err(eyre::Error::from)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DeletedFile {
    id: String,
}

impl DeletedFile {
    #[instrument(skip_all)]
    pub(crate) async fn send<C>(self, device: &mut C) -> eyre::Result<()>
    where
        C: astarte_device_sdk::Client + Send + Sync + 'static,
    {
        device
            .unset_property(INTERFACE, &format!("/{}/pathOnDevice", self.id))
            .await?;

        device
            .unset_property(INTERFACE, &format!("/{}/sizeBytes", self.id))
            .await
            .map_err(eyre::Error::from)
    }
}
