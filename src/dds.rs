use std::{
    convert::{TryFrom, TryInto},
    error::Error,
};

use ddsfile::{AlphaMode, Caps2, D3D10ResourceDimension, Dds, DxgiFormat, NewDxgiParams};

use crate::{BntxFile, SurfaceFormat};

pub fn create_dds(bntx: &BntxFile) -> Result<Dds, Box<dyn Error>> {
    let some_if_above_one = |x| if x > 0 { Some(x) } else { None };

    let mut dds = Dds::new_dxgi(NewDxgiParams {
        height: bntx.nx_header.info_ptr.height,
        width: bntx.nx_header.info_ptr.width,
        depth: some_if_above_one(bntx.nx_header.info_ptr.depth),
        format: bntx.nx_header.info_ptr.format.into(),
        mipmap_levels: some_if_above_one(bntx.nx_header.info_ptr.mipmap_count as u32),
        array_layers: some_if_above_one(bntx.nx_header.info_ptr.layer_count),
        caps2: if bntx.nx_header.info_ptr.depth > 1 {
            Some(Caps2::VOLUME)
        } else {
            None
        },
        is_cubemap: bntx.nx_header.info_ptr.layer_count == 6,
        // TODO: Check the dimension instead?
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

// TODO: Make this a method?
pub fn create_bntx(name: &str, dds: &Dds) -> Result<BntxFile, Box<dyn Error>> {
    // TODO: Use more robust code for getting the format.
    BntxFile::from_image_data(
        name,
        dds.get_width(),
        dds.get_height(),
        dds.get_depth(),
        dds.get_num_mipmap_levels(),
        dds.get_num_array_layers(),
        dds.get_dxgi_format()
            .ok_or("Only DXGI DDS files are supported".to_string())?
            .try_into()?,
        &dds.data,
    )
}

impl From<SurfaceFormat> for DxgiFormat {
    fn from(f: SurfaceFormat) -> Self {
        match f {
            SurfaceFormat::R8Unorm => Self::R8_UNorm,
            SurfaceFormat::R8B8G8A8Unorm => Self::R8G8B8A8_UNorm,
            SurfaceFormat::R8G8B8A8Srgb => Self::R8G8B8A8_UNorm_sRGB,
            SurfaceFormat::B8G8R8A8Unorm => Self::B8G8R8A8_UNorm,
            SurfaceFormat::B8G8R8A8Srgb => Self::B8G8R8A8_UNorm_sRGB,
            SurfaceFormat::BC1Unorm => Self::BC1_UNorm,
            SurfaceFormat::BC1Srgb => Self::BC1_UNorm_sRGB,
            SurfaceFormat::BC2Unorm => Self::BC2_UNorm,
            SurfaceFormat::BC2Srgb => Self::BC2_UNorm_sRGB,
            SurfaceFormat::BC3Unorm => Self::BC3_UNorm,
            SurfaceFormat::BC3Srgb => Self::BC3_UNorm_sRGB,
            SurfaceFormat::BC4Unorm => Self::BC4_UNorm,
            SurfaceFormat::BC4Snorm => Self::BC4_SNorm,
            SurfaceFormat::BC5Unorm => Self::BC5_UNorm,
            SurfaceFormat::BC5Snorm => Self::BC5_SNorm,
            SurfaceFormat::BC6Sfloat => Self::BC6H_SF16,
            SurfaceFormat::BC6Ufloat => Self::BC6H_UF16,
            SurfaceFormat::BC7Unorm => Self::BC7_UNorm,
            SurfaceFormat::BC7Srgb => Self::BC7_UNorm_sRGB,
        }
    }
}

impl TryFrom<DxgiFormat> for SurfaceFormat {
    type Error = String;

    fn try_from(value: DxgiFormat) -> Result<Self, Self::Error> {
        match value {
            DxgiFormat::R8_UNorm => Ok(SurfaceFormat::R8Unorm),
            DxgiFormat::R8G8B8A8_UNorm => Ok(SurfaceFormat::R8B8G8A8Unorm),
            DxgiFormat::R8G8B8A8_UNorm_sRGB => Ok(SurfaceFormat::R8G8B8A8Srgb),
            DxgiFormat::B8G8R8A8_UNorm => Ok(SurfaceFormat::B8G8R8A8Unorm),
            DxgiFormat::B8G8R8A8_UNorm_sRGB => Ok(SurfaceFormat::B8G8R8A8Srgb),
            DxgiFormat::BC1_UNorm => Ok(SurfaceFormat::BC1Unorm),
            DxgiFormat::BC1_UNorm_sRGB => Ok(SurfaceFormat::BC1Srgb),
            DxgiFormat::BC2_UNorm => Ok(SurfaceFormat::BC2Unorm),
            DxgiFormat::BC2_UNorm_sRGB => Ok(SurfaceFormat::BC2Srgb),
            DxgiFormat::BC3_UNorm => Ok(SurfaceFormat::BC3Unorm),
            DxgiFormat::BC3_UNorm_sRGB => Ok(SurfaceFormat::BC3Srgb),
            DxgiFormat::BC4_UNorm => Ok(SurfaceFormat::BC4Unorm),
            DxgiFormat::BC4_SNorm => Ok(SurfaceFormat::BC4Snorm),
            DxgiFormat::BC5_UNorm => Ok(SurfaceFormat::BC5Unorm),
            DxgiFormat::BC5_SNorm => Ok(SurfaceFormat::BC5Snorm),
            DxgiFormat::BC6H_SF16 => Ok(SurfaceFormat::BC6Sfloat),
            DxgiFormat::BC6H_UF16 => Ok(SurfaceFormat::BC6Ufloat),
            DxgiFormat::BC7_UNorm => Ok(SurfaceFormat::BC7Unorm),
            DxgiFormat::BC7_UNorm_sRGB => Ok(SurfaceFormat::BC7Srgb),
            _ => Err(format!(
                "DDS DXGI format {:?} does not have a corresponding Nutexb format.",
                value
            )),
        }
    }
}
