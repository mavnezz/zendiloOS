//! The image library: ISOs kept once in the VMs' storage, added from a
//! share (copied in a job) or uploaded from a browser, renamed, and removed
//! while no VM has them in a drive.

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, Outcome, RequestContext};

use super::{HostError, JobSteps, Vms, VmsError, job_error, outcome};
use crate::domain::{IMAGE_UPLOAD_MAX, ImageOrigin, Invalid, IsoChoice, VmImage, check_image_name};

/// An image, and the VMs that have it in a drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmImageView {
    pub image: VmImage,
    pub used_by: Vec<String>,
}

/// An upload begun: what is uploaded goes to `file`, which is closed once
/// all is written; then the ticket finishes it, or gives it up.
#[derive(Debug)]
pub struct ImageUpload {
    pub file: tokio::fs::File,
    pub ticket: UploadTicket,
}

/// The image an upload makes.
#[derive(Debug)]
pub struct UploadTicket {
    image: VmImage,
}

impl UploadTicket {
    /// The bytes announced, which the upload must have exactly.
    pub fn bytes(&self) -> u64 {
        self.image.bytes.unwrap_or(0)
    }
}

impl Vms {
    /// The library, oldest first, with the VMs each image is in. The size
    /// of an image whose size is not known (one a VM had before the
    /// library) is looked at and kept.
    pub async fn images(&self, context: &RequestContext) -> Result<Vec<VmImageView>, VmsError> {
        let mut images = self.ports.images.list().await?;
        if images.iter().any(|image| image.ready && image.bytes.is_none())
            && let Ok((_, place)) = self.storage(context).await
        {
            match self.ports.host.image_files(&place, context).await {
                Ok(files) => {
                    for image in images.iter_mut().filter(|image| image.bytes.is_none()) {
                        if let Some(bytes) = files.get(&image.id) {
                            self.ports.images.set_bytes(&image.id, *bytes).await?;
                            image.bytes = Some(*bytes);
                        }
                    }
                }
                Err(error) => tracing::warn!(%error, "vms: cannot look at the library's files"),
            }
        }
        let vms = self.ports.repository.list().await?;
        Ok(images
            .into_iter()
            .map(|image| {
                let used_by = vms
                    .iter()
                    .filter(|vm| vm.settings.images().any(|held| held == image.id))
                    .map(|vm| vm.settings.name.clone())
                    .collect();
                VmImageView { image, used_by }
            })
            .collect())
    }

    async fn image(&self, id: &str) -> Result<VmImage, VmsError> {
        if !crate::domain::is_vm_id(id) {
            return Err(VmsError::NotFound);
        }
        self.ports.images.find(id).await?.ok_or(VmsError::NotFound)
    }

    /// Records an image and copies it from a share into the library in a
    /// job: the file opened as the administrator first, so a file they may
    /// not read is refused. It is called `name`, else after its file.
    pub async fn import_image(
        &self,
        choice: IsoChoice,
        name: Option<&str>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(VmImage, Job), VmsError> {
        let (_, place) = self.storage(context).await?;
        choice.check()?;
        let name = check_image_name(name.unwrap_or_else(|| choice.file()))?;
        let source = self.ports.isos.locate(actor, &choice, context).await?;
        let image = VmImage {
            id: self.new_id().await?,
            name,
            bytes: None,
            origin: ImageOrigin::Share { share: source.share.clone(), path: source.path.clone() },
            volume: place.volume,
            added_at: self.ports.clock.now(),
            ready: false,
        };
        if !self.ports.images.create(&image).await? {
            return Err(VmsError::NameTaken);
        }
        let this = self.clone();
        let (who, context_owned, added) = (actor.clone(), context.clone(), image.clone());
        let started = self
            .ports
            .jobs
            .start("vms.image_add", Some(image.name.clone()), Some(actor), move |progress| async move {
                progress.step("copying_iso", Some(0.0)).await;
                // Reported as the agent's answer comes, kept in order.
                let (sender, mut reported) = tokio::sync::mpsc::unbounded_channel::<(String, Option<u8>)>();
                let forwarded = {
                    let progress = progress.clone();
                    tokio::spawn(async move {
                        while let Some((step, percent)) = reported.recv().await {
                            let share = percent.map(|p| f32::from(p.min(100)) / 100.0);
                            progress.step(&step, share).await;
                        }
                    })
                };
                let host = &this.ports.host;
                let copied =
                    host.copy_image(&added.id, &place, &source, &JobSteps(sender), &context_owned).await;
                let _ = forwarded.await;
                let done = match copied {
                    Ok(bytes) => this.ports.images.set_ready(&added.id, bytes).await.map_err(HostError::from),
                    Err(error) => Err(error),
                };
                if done.is_err() {
                    this.forget_image(&added, &context_owned).await;
                }
                this.audit("vms.image_add", outcome(&done), &added.name, &who, &context_owned).await;
                done.map_err(job_error)
            })
            .await;
        match started {
            Ok(job) => Ok((image, job)),
            Err(error) => {
                let _ = self.ports.images.delete(&image.id).await;
                Err(error.into())
            }
        }
    }

    /// An image that was not added goes: its file, if any, and its record.
    async fn forget_image(&self, image: &VmImage, context: &RequestContext) {
        if let Err(error) = self.ports.host.delete_image(&image.id, image.volume, context).await {
            tracing::warn!(%error, id = image.id, "vms: cannot remove what an image not added left");
        }
        if let Err(error) = self.ports.images.delete(&image.id).await {
            tracing::error!(%error, id = image.id, "vms: cannot forget an image not added");
        }
    }

    /// Begins an upload of `bytes` from a browser into the library, at
    /// most [`IMAGE_UPLOAD_MAX`], as `name`.
    pub async fn begin_upload(
        &self,
        name: &str,
        bytes: u64,
        context: &RequestContext,
    ) -> Result<ImageUpload, VmsError> {
        let (_, place) = self.storage(context).await?;
        if bytes == 0 || bytes > IMAGE_UPLOAD_MAX {
            return Err(
                Invalid(format!("an uploaded image has 1 byte to {} GiB", IMAGE_UPLOAD_MAX >> 30)).into()
            );
        }
        let name = check_image_name(name)?;
        let image = VmImage {
            id: self.new_id().await?,
            origin: ImageOrigin::Upload { file: name.clone() },
            name,
            bytes: Some(bytes),
            volume: place.volume,
            added_at: self.ports.clock.now(),
            ready: false,
        };
        if !self.ports.images.create(&image).await? {
            return Err(VmsError::NameTaken);
        }
        match self.ports.host.open_upload(&image.id, image.volume, bytes, context).await {
            Ok(file) => Ok(ImageUpload { file, ticket: UploadTicket { image } }),
            Err(error) => {
                let _ = self.ports.images.delete(&image.id).await;
                Err(error.into())
            }
        }
    }

    /// Once all was written and the upload's file closed: the image is in
    /// the library, or gone again.
    pub async fn finish_upload(
        &self,
        ticket: UploadTicket,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmImage, VmsError> {
        let image = ticket.image;
        let stored = self.ports.host.finish_upload(&image.id, context).await;
        let done = match stored {
            Ok(bytes) => {
                self.ports.images.set_ready(&image.id, bytes).await.map(|()| bytes).map_err(VmsError::from)
            }
            Err(error) => Err(error.into()),
        };
        self.audit("vms.image_upload", outcome(&done), &image.name, actor, context).await;
        match done {
            Ok(bytes) => Ok(VmImage { bytes: Some(bytes), ready: true, ..image }),
            Err(error) => {
                self.forget_image(&image, context).await;
                Err(error)
            }
        }
    }

    /// An upload that did not come whole, its file closed: nothing of it
    /// stays.
    pub async fn abort_upload(&self, ticket: UploadTicket, actor: &Actor, context: &RequestContext) {
        let image = ticket.image;
        // Its unit ends on the short pipe; its answer is not needed.
        let _ = self.ports.host.finish_upload(&image.id, context).await;
        self.forget_image(&image, context).await;
        self.audit("vms.image_upload", Outcome::Failure, &image.name, actor, context).await;
    }

    pub async fn rename_image(
        &self,
        id: &str,
        name: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmImage, VmsError> {
        let image = self.image(id).await?;
        let name = check_image_name(name)?;
        let renamed = self.ports.images.rename(&image.id, &name).await;
        self.audit("vms.image_rename", outcome(&renamed), &name, actor, context).await;
        if !renamed? {
            return Err(VmsError::NameTaken);
        }
        Ok(VmImage { name, ..image })
    }

    /// Removes an image from the library; refused while a VM has it in a
    /// drive, naming the VMs, or while it is being added.
    pub async fn delete_image(
        &self,
        id: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), VmsError> {
        let image = self.image(id).await?;
        let _library = self.library.lock().await;
        let users: Vec<String> = self
            .ports
            .repository
            .list()
            .await?
            .into_iter()
            .filter(|vm| vm.settings.images().any(|held| held == image.id))
            .map(|vm| vm.settings.name)
            .collect();
        if !users.is_empty() {
            return Err(VmsError::ImageInUse(users));
        }
        if !image.ready {
            return Err(VmsError::Refused("the image is still being added".into()));
        }
        let removed = self.ports.host.delete_image(&image.id, image.volume, context).await;
        self.audit("vms.image_delete", outcome(&removed), &image.name, actor, context).await;
        removed?;
        self.ports.images.delete(&image.id).await?;
        Ok(())
    }

    /// Images whose copy or upload was cut off when zendiloosd stopped go,
    /// with what of them is on the host. Says how many.
    pub async fn discard_unfinished_images(&self, context: &RequestContext) -> Result<usize, VmsError> {
        let mut removed = 0;
        for image in self.ports.images.list().await?.into_iter().filter(|image| !image.ready) {
            self.forget_image(&image, context).await;
            removed += 1;
        }
        Ok(removed)
    }
}
