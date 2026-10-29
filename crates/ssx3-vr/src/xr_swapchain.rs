//! Temporary Vulkan swapchain path: two different eye colors prove that the
//! OpenXR compositor receives both array layers. Game textures replace this.

use std::error::Error;

use ash::vk::{self, Handle};
use openxr as xr;

pub struct XrColorTarget<'a> {
    swapchain: xr::Swapchain<xr::Vulkan>,
    device: &'a ash::Device,
    images: Vec<vk::Image>,
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    queue: vk::Queue,
    width: u32,
    height: u32,
}

impl<'a> XrColorTarget<'a> {
    pub fn new(
        instance: &xr::Instance,
        system: xr::SystemId,
        session: &xr::Session<xr::Vulkan>,
        device: &'a ash::Device,
        family: u32,
    ) -> Result<Self, Box<dyn Error>> {
        let views = instance.enumerate_view_configuration_views(
            system,
            xr::ViewConfigurationType::PRIMARY_STEREO,
        )?;
        if views.len() != 2 || views[0] != views[1] {
            return Err("Expected two equal-sized OpenXR stereo views".into());
        }
        let width = views[0].recommended_image_rect_width;
        let height = views[0].recommended_image_rect_height;
        let formats = session.enumerate_swapchain_formats()?;
        let format = [
            vk::Format::R8G8B8A8_SRGB,
            vk::Format::B8G8R8A8_SRGB,
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::B8G8R8A8_UNORM,
        ]
        .into_iter()
        .find(|f| formats.contains(&(f.as_raw() as _)))
        .ok_or("No usable RGBA8/BGRA8 OpenXR swapchain format")?;
        let swapchain = session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                | xr::SwapchainUsageFlags::TRANSFER_DST,
            format: format.as_raw() as _,
            sample_count: 1,
            width,
            height,
            face_count: 1,
            array_size: 2,
            mip_count: 1,
        })?;
        let images = swapchain
            .enumerate_images()?
            .into_iter()
            .map(vk::Image::from_raw)
            .collect::<Vec<_>>();
        if images.is_empty() {
            return Err("OpenXR returned no swapchain images".into());
        }
        println!(
            "OpenXR swapchain: {width}x{height}x2, {format:?}, {} images",
            images.len()
        );
        let pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };
        let command = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )?[0]
        };
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None)? };
        let queue = unsafe { device.get_device_queue(family, 0) };
        Ok(Self {
            swapchain,
            device,
            images,
            pool,
            command,
            fence,
            queue,
            width,
            height,
        })
    }

    pub fn clear_eyes(&mut self) -> Result<(), Box<dyn Error>> {
        let image_index = self.swapchain.acquire_image()? as usize;
        self.swapchain.wait_image(xr::Duration::INFINITE)?;
        let image = self.images[image_index];
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 2,
        };
        unsafe {
            self.device
                .reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty())?;
            self.device.begin_command_buffer(
                self.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let to_transfer = vk::ImageMemoryBarrier::default()
                .image(image)
                .subresource_range(range)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            self.device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            for eye in 0..2 {
                let eye_range = vk::ImageSubresourceRange {
                    base_array_layer: eye,
                    layer_count: 1,
                    ..range
                };
                let color = vk::ClearColorValue {
                    float32: if eye == 0 {
                        [0.16, 0.02, 0.02, 1.0]
                    } else {
                        [0.02, 0.02, 0.16, 1.0]
                    },
                };
                self.device.cmd_clear_color_image(
                    self.command,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &color,
                    &[eye_range],
                );
            }
            let to_color = vk::ImageMemoryBarrier::default()
                .image(image)
                .subresource_range(range)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ);
            self.device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_color],
            );
            self.device.end_command_buffer(self.command)?;
            let commands = [self.command];
            self.device.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&commands)],
                self.fence,
            )?;
            self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;
            self.device.reset_fences(&[self.fence])?;
        }
        self.swapchain.release_image()?;
        Ok(())
    }

    pub fn projection_views(
        &self,
        views: &[xr::View],
    ) -> Result<[xr::CompositionLayerProjectionView<'_, xr::Vulkan>; 2], Box<dyn Error>> {
        if views.len() != 2 {
            return Err(format!("Expected 2 OpenXR views, got {}", views.len()).into());
        }
        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: self.width as _,
                height: self.height as _,
            },
        };
        Ok(std::array::from_fn(|eye| {
            xr::CompositionLayerProjectionView::new()
                .pose(views[eye].pose)
                .fov(views[eye].fov)
                .sub_image(
                    xr::SwapchainSubImage::new()
                        .swapchain(&self.swapchain)
                        .image_array_index(eye as u32)
                        .image_rect(rect),
                )
        }))
    }
}

impl Drop for XrColorTarget<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.pool, None);
        }
    }
}
