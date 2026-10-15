//! Cloud object storage endpoint declarations.
//!
//! This is a configuration and safety-validation layer, not an HTTP client.
//! Provider adapters must authenticate using ambient credentials and must
//! implement conditional immutable writes before being enabled for writes.
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudProvider {
    AwsS3,
    AzureBlob,
    GoogleCloudStorage,
    S3Compatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudObjectLocation {
    pub provider: CloudProvider,
    /// S3 bucket, Azure container, or Google Cloud Storage bucket.
    pub namespace: String,
    /// Optional object key prefix. Never begins with a slash.
    pub prefix: String,
    /// Custom HTTPS endpoint, primarily for S3-compatible implementations.
    pub endpoint: Option<String>,
}

impl CloudObjectLocation {
    pub fn validate(&self) -> Result<()> {
        if self.namespace.is_empty()
            || self.namespace.len() > 255
            || !self.namespace.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(Error::Invalid("invalid cloud object namespace".into()));
        }
        if self.prefix.starts_with('/')
            || self.prefix.contains('\\')
            || self.prefix.split('/').any(|segment| segment == "." || segment == "..")
            || self.prefix.chars().any(char::is_control)
        {
            return Err(Error::Invalid("invalid cloud object prefix".into()));
        }
        if let Some(endpoint) = &self.endpoint {
            if self.provider != CloudProvider::S3Compatible {
                return Err(Error::Invalid("custom endpoint requires S3-compatible provider".into()));
            }
            if !endpoint.starts_with("https://")
                || endpoint.contains('@')
                || endpoint.contains('?')
                || endpoint.contains('#')
                || endpoint.chars().any(char::is_whitespace)
            {
                return Err(Error::Invalid("cloud endpoint must be an unambiguous HTTPS origin".into()));
            }
            let origin = &endpoint["https://".len()..];
            if origin.is_empty() || origin.contains('/') || origin.starts_with(':') {
                return Err(Error::Invalid("cloud endpoint must be an HTTPS origin without a path".into()));
            }
        }
        Ok(())
    }

    /// Return the canonical relative object key. Object IDs are independent
    /// of provider, bucket and prefix; moving backends does not change IDs.
    pub fn object_key(&self, id: crate::ObjectId) -> Result<String> {
        self.validate()?;
        let hex = id.to_string();
        let key = format!("{}/{}/{}", "objects", &hex[..2], &hex[2..]);
        if self.prefix.is_empty() {
            Ok(key)
        } else {
            Ok(format!("{}/{key}", self.prefix.trim_end_matches('/')))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_neutral_keys_are_stable() {
        let id = crate::ObjectId([7; 32]);
        let mut config = CloudObjectLocation {
            provider: CloudProvider::AwsS3,
            namespace: "loom-objects".into(),
            prefix: "team/repo".into(),
            endpoint: None,
        };
        let expected = config.object_key(id).unwrap();
        config.provider = CloudProvider::AzureBlob;
        assert_eq!(config.object_key(id).unwrap(), expected);
        config.provider = CloudProvider::GoogleCloudStorage;
        assert_eq!(config.object_key(id).unwrap(), expected);
        config.provider = CloudProvider::S3Compatible;
        config.endpoint = Some("https://minio.example.org".into());
        assert_eq!(config.object_key(id).unwrap(), expected);
    }

    #[test]
    fn rejects_unsafe_cloud_configuration() {
        let mut config = CloudObjectLocation {
            provider: CloudProvider::AwsS3,
            namespace: "loom-objects".into(),
            prefix: "../escape".into(),
            endpoint: None,
        };
        assert!(config.validate().is_err());
        config.prefix = "team".into();
        config.endpoint = Some("http://localhost:9000".into());
        assert!(config.validate().is_err());
        config.provider = CloudProvider::S3Compatible;
        config.endpoint = Some("https://user:secret@host".into());
        assert!(config.validate().is_err());
    }
}
