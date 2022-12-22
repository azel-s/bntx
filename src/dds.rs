use std::error::Error;

use ddsfile::{AlphaMode, Caps2, D3D10ResourceDimension, Dds, DxgiFormat, NewDxgiParams};

use crate::{BntxFile, SurfaceFormat};

pub fn create_dds(bntx: &BntxFile) -> Result<Dds, Box<dyn Error>> {
    let some_if_above_one = |x| if x > 0 { Some(x) } else { None };

    let mut dds = Dds::new_dxgi(NewDxgiParams {
        height: bntx.nx_header.info_ptr.height,
        width: bntx.nx_header.info_ptr.width,
        depth: some_if_above_one(bntx.nx_header.info_ptr.depth),
        format: bntx.nx_header.info_ptr.format.into(),
        mipmap_levels: some_if_above_one(bntx.nx_header.info_ptr.mips_count as u32),
        array_layers: some_if_above_one(bntx.nx_header.info_ptr.layer_count),
        caps2: if bntx.nx_header.info_ptr.depth > 1 {
            Some(Caps2::VOLUME)
        } else {
            None
        },
        is_cubemap: bntx.nx_header.info_ptr.layer_count == 6,
        resource_dimension: if bntx.nx_header.info_ptr.depth > 1 {
            D3D10ResourceDimension::Texture3D
        } else {
            D3D10ResourceDimension::Texture2D
        },
        alpha_mode: AlphaMode::Unknown, // TODO: Alpha mode?
    })
    .unwrap();

    // DDS stores mipmaps in a contiguous region of memory.
    dds.data = bntx.deswizzled_data()?;

    Ok(dds)
}

// TODO: Support the other conversion direction.

impl From<SurfaceFormat> for DxgiFormat {
    fn from(f: SurfaceFormat) -> Self {
        match f {
            SurfaceFormat::R8G8B8A8Srgb => DxgiFormat::R8G8B8A8_UNorm_sRGB,
            SurfaceFormat::BC7Unorm => DxgiFormat::BC7_UNorm,
        }
    }
}
