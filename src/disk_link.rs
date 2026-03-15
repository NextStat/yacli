use std::path::PathBuf;

use serde::Serialize;

use crate::disk::{
    DiskResource, DiskUploadReview, PrivateDiskPublishRequest, PrivateDiskUploadRequest,
    UploadedFile, publish_private_resource, review_private_upload, upload_private_resource,
};
use crate::error::{Result, YacliError};

const PUBLIC_LINK_PLACEHOLDER: &str = "<публичная ссылка на файл>";

#[derive(Clone, Debug)]
pub struct DiskUploadLinkRequest {
    pub source: PathBuf,
    pub disk_path: String,
    pub overwrite: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskUploadLinkReview {
    pub upload: DiskUploadReview,
    pub publish_path: String,
    pub link_placeholder: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskUploadLinkResult {
    pub upload: UploadedFile,
    pub resource: DiskResource,
}

pub fn review_disk_upload_link(request: &DiskUploadLinkRequest) -> Result<DiskUploadLinkReview> {
    let upload = review_private_upload(&PrivateDiskUploadRequest {
        source: request.source.clone(),
        path: request.disk_path.clone(),
        overwrite: request.overwrite,
    })?;

    Ok(DiskUploadLinkReview {
        publish_path: request.disk_path.clone(),
        upload,
        link_placeholder: PUBLIC_LINK_PLACEHOLDER.to_string(),
    })
}

pub fn upload_link_to_disk(
    disk_base_url: &str,
    access_token: &str,
    request: &DiskUploadLinkRequest,
) -> Result<DiskUploadLinkResult> {
    let (_, upload) = upload_private_resource(
        disk_base_url,
        access_token,
        &PrivateDiskUploadRequest {
            source: request.source.clone(),
            path: request.disk_path.clone(),
            overwrite: request.overwrite,
        },
    )?;
    let resource = publish_private_resource(
        disk_base_url,
        access_token,
        &PrivateDiskPublishRequest {
            path: request.disk_path.clone(),
        },
    )?;

    if resource.public_url.is_none() && resource.public_key.is_none() {
        return Err(YacliError::Api(format!(
            "Yandex Disk did not return public_url/public_key after publishing `{}`",
            resource.path
        )));
    }

    Ok(DiskUploadLinkResult { upload, resource })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn review_disk_upload_link_uses_placeholder_link_without_network() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("archive.zip");
        std::fs::write(&source, "archive-body").expect("source");

        let review = review_disk_upload_link(&DiskUploadLinkRequest {
            source,
            disk_path: "disk:/docs/archive.zip".to_string(),
            overwrite: false,
        })
        .expect("review");

        assert_eq!(review.upload.remote_path, "disk:/docs/archive.zip");
        assert_eq!(review.publish_path, "disk:/docs/archive.zip");
        assert_eq!(review.link_placeholder, PUBLIC_LINK_PLACEHOLDER);
    }
}
