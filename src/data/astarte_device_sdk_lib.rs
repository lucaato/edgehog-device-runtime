/*
 * This file is part of Edgehog.
 *
 * Copyright 2022 SECO Mind Srl
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *   http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
 * SPDX-License-Identifier: Apache-2.0
 */

use std::path::Path;

use astarte_device_sdk::builder::DeviceBuilder;
use astarte_device_sdk::store::memory::MemoryStore;
use astarte_device_sdk::transport::mqtt::{registration, Mqtt, MqttConfig};
use astarte_device_sdk::types::AstarteType;
use astarte_device_sdk::{
    error::Error as AstarteError, AstarteAggregate, AstarteDeviceDataEvent, AstarteDeviceSdk,
};
use astarte_device_sdk::{prelude::*, EventReceiver};
use async_trait::async_trait;
use serde::Deserialize;

use crate::data::{Publisher, Subscriber};
use crate::device::DeviceProxy;
use crate::error::DeviceManagerError;
use crate::repository::file_state_repository::FileStateRepository;
use crate::repository::StateRepository;
use crate::DeviceManagerOptions;

/// Error returned by the [`astarte_device_sdk`].
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum DeviceSdkError {
    /// missing configuration for the Astarte SDK
    MissingConfig,
    /// missing device ID
    MissingDeviceId,
    /// couldn't get the hardware id from DBus
    Zbus(#[from] zbus::Error),
    /// couldn't pair device to Astarte
    Pairing(#[from] astarte_device_sdk::transport::mqtt::PairingError),
    /// couldn't write credential secret
    WriteSecret(#[source] DeviceManagerError),
    /// couldn't read credential secret
    ReadSecret(#[source] DeviceManagerError),
    /// couldn't get credential secret or pairing token
    MissingCredentialSecret,
    /// couldn't add interfaces directory
    Interfaces(#[source] astarte_device_sdk::builder::BuilderError),
    /// couldn't connect to Astarte
    Connect(#[source] astarte_device_sdk::Error),
}

#[derive(Debug, Deserialize, Clone)]
pub struct AstarteDeviceSdkConfigOptions {
    pub realm: String,
    pub device_id: Option<String>,
    pub credentials_secret: Option<String>,
    pub pairing_url: String,
    pub pairing_token: Option<String>,
    #[serde(default)]
    pub ignore_ssl: bool,
}

impl AstarteDeviceSdkConfigOptions {
    async fn device_id_or_from_dbus(&self) -> Result<String, DeviceSdkError> {
        if let Some(id) = self.device_id.as_ref().filter(|id| !id.is_empty()) {
            return Ok(id.clone());
        }

        hardware_id_from_dbus()
            .await?
            .ok_or(DeviceSdkError::MissingDeviceId)
    }

    async fn credentials_secret(
        &self,
        device_id: &str,
        store_directory: impl AsRef<Path>,
    ) -> Result<String, DeviceSdkError> {
        let cred = self.credentials_secret.as_ref().filter(|id| !id.is_empty());

        if let Some(secret) = cred {
            return Ok(secret.clone());
        }

        let registry = FileStateRepository::new(
            store_directory.as_ref(),
            format!("credentials_{}.json", device_id),
        );

        if StateRepository::<String>::exists(&registry).await {
            return registry.read().await.map_err(DeviceSdkError::ReadSecret);
        }

        if let Some(token) = &self.pairing_token {
            return self.register_device(device_id, token, registry).await;
        }

        Err(DeviceSdkError::MissingCredentialSecret)
    }

    async fn register_device(
        &self,
        device_id: &str,
        token: &str,
        cred_state_repo: impl StateRepository<String>,
    ) -> Result<String, DeviceSdkError> {
        let credential_secret =
            registration::register_device(token, &self.pairing_url, &self.realm, device_id).await?;

        cred_state_repo
            .write(&credential_secret)
            .await
            .map_err(DeviceSdkError::WriteSecret)?;

        Ok(credential_secret)
    }
}

pub async fn hardware_id_from_dbus() -> Result<Option<String>, DeviceSdkError> {
    let connection = zbus::Connection::system().await?;
    let proxy = DeviceProxy::new(&connection).await?;
    let hardware_id: String = proxy.get_hardware_id("").await?;

    if hardware_id.is_empty() {
        return Ok(None);
    }

    Ok(Some(hardware_id))
}

#[derive(Clone)]
pub struct AstarteDeviceSdkLib {
    pub device: AstarteDeviceSdk<MemoryStore, Mqtt>,
    pub rx: EventReceiver,
}

impl AstarteDeviceSdkLib {
    pub async fn connect(options: &DeviceManagerOptions) -> Result<Self, DeviceSdkError> {
        let sdk_opts = options
            .astarte_device_sdk
            .as_ref()
            .ok_or(DeviceSdkError::MissingConfig)?;

        let device_id = sdk_opts.device_id_or_from_dbus().await?;

        let credentials_secret = sdk_opts
            .credentials_secret(&device_id, &options.store_directory)
            .await?;

        let mut mqtt_cfg = MqttConfig::new(
            &sdk_opts.realm,
            &device_id,
            &credentials_secret,
            &sdk_opts.pairing_url,
        );

        if sdk_opts.ignore_ssl {
            mqtt_cfg.ignore_ssl_errors();
        }

        let (device, rx) = DeviceBuilder::new()
            .store(MemoryStore::new())
            .interface_directory(&options.interfaces_directory)
            .map_err(DeviceSdkError::Interfaces)?
            .connect(mqtt_cfg)
            .await
            .map_err(DeviceSdkError::Connect)?
            .build();

        Ok(Self { device, rx })
    }
}

#[async_trait]
impl Publisher for AstarteDeviceSdkLib {
    async fn send_object<T: 'static>(
        &self,
        interface_name: &str,
        interface_path: &str,
        data: T,
    ) -> Result<(), AstarteError>
    where
        T: AstarteAggregate + Send,
    {
        self.device_sdk
            .send_object(interface_name, interface_path, data)
            .await
    }

    async fn send(
        &self,
        interface_name: &str,
        interface_path: &str,
        data: AstarteType,
    ) -> Result<(), AstarteError> {
        self.device_sdk
            .send(interface_name, interface_path, data)
            .await
    }
}

#[async_trait]
impl Subscriber for AstarteDeviceSdkLib {
    async fn on_event(&mut self) -> Result<AstarteDeviceDataEvent, AstarteError> {
        self.rx.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use astarte_device_sdk::transport::mqtt::PairingError;
    use tempdir::TempDir;

    use crate::data::astarte_device_sdk_lib::AstarteDeviceSdkConfigOptions;
    use crate::repository::MockStateRepository;

    #[tokio::test]
    async fn device_id_test() {
        let opts = AstarteDeviceSdkConfigOptions {
            realm: "foo".to_string(),
            device_id: Some("target".to_string()),
            credentials_secret: None,
            pairing_url: None,
            pairing_token: None,
            ignore_ssl: false,
        };

        let id = opts.device_id_or_from_dbus().await.unwrap();
        assert_eq!(id, "target");
    }

    #[tokio::test]
    async fn credentials_secret_test() {
        let _dir = TempDir::new("sdk_cred").unwrap();
        let path = _dir.path().to_owned();

        let options = AstarteDeviceSdkConfigOptions {
            realm: "".to_string(),
            device_id: None,
            credentials_secret: Some("credentials_secret".to_string()),
            pairing_url: "".to_string(),
            pairing_token: None,
            ignore_ssl: false,
        };

        let secret = options.credentials_secret("device_id", path).await.unwrap();

        assert_eq!(secret, "credentials_secret");
    }

    #[tokio::test]
    async fn not_enough_arguments_credentials_secret_test() {
        let _dir = TempDir::new("sdk_cred").unwrap();
        let path = _dir.path().to_owned();

        let options = AstarteDeviceSdkConfigOptions {
            realm: "".to_string(),
            device_id: None,
            credentials_secret: None,
            pairing_url: "".to_string(),
            pairing_token: None,
            ignore_ssl: false,
        };

        let res = options.credentials_secret("device_id", &path).await;

        assert!(res.is_err());
    }

    #[tokio::test]
    async fn get_credentials_secret_persistence_fail() {
        let _dir = TempDir::new("sdk_cred").unwrap();
        let path = _dir.path().to_owned();

        let device_id = "device_id";

        let path = path.join(format!("credentials_{}.json", device_id));

        tokio::fs::write(&path, b"\0");

        let options = AstarteDeviceSdkConfigOptions {
            realm: "".to_string(),
            device_id: Some(device_id.to_owned()),
            credentials_secret: None,
            pairing_url: "".to_string(),
            pairing_token: None,
            ignore_ssl: true,
        };

        let res = options.credentials_secret(device_id, path).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn get_credentials_secret_persistence_success() {
        let _dir = TempDir::new("sdk_cred").unwrap();
        let path = _dir.path().to_owned();

        let device_id = "device_id";

        let path = path.join(format!("credentials_{}.json", device_id));

        tokio::fs::write(&path, "\"credential_secret\"");

        let options = AstarteDeviceSdkConfigOptions {
            realm: "".to_string(),
            device_id: Some(device_id.to_owned()),
            credentials_secret: None,
            pairing_url: "".to_string(),
            pairing_token: None,
            ignore_ssl: false,
        };

        let secret = options.credentials_secret(device_id, path).await.unwrap();

        assert_eq!(secret, "credentials_secret");
    }

    #[tokio::test]
    async fn get_credentials_secret_from_registration_fail() {
        let device_id = "device_id";
        let token = "token";
        let options = AstarteDeviceSdkConfigOptions {
            realm: String::new(),
            device_id: Some(device_id.to_owned()),
            credentials_secret: Some("credentials_secret".to_string()),
            pairing_url: String::new(),
            pairing_token: Some(token.to_string()),
            ignore_ssl: false,
        };

        let state_mock = MockStateRepository::<String>::new();
        let cred_result = options.register_device(device_id, token, state_mock).await;

        assert!(matches!(
            cred_result,
            Err(DeviceSdkError::Pairing(PairingError::InvalidUrl(_)))
        ));
    }
}
