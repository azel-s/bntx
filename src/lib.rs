use binrw::binread;
use binrw::binrw;
use binrw::prelude::*;
use binrw::BinWrite;
use binrw::{FilePtr16, FilePtr32, FilePtr64, NullString};
use std::convert::TryFrom;
use std::io::SeekFrom;
use std::path::Path;
use std::{fmt, io};
use tegra_swizzle::block_height_mip0;
use tegra_swizzle::div_round_up;
use tegra_swizzle::mip_block_height;
use tegra_swizzle::surface::{deswizzle_surface, swizzle_surface, BlockDim};
use tegra_swizzle::BlockHeight;

// TODO: Add module level docs for basic usage.
// TODO: Make this optional.
pub mod dds;

const BNTX_HEADER_SIZE: usize = 0x20;
const NX_HEADER_SIZE: usize = 0x28;
const HEADER_SIZE: usize = BNTX_HEADER_SIZE + NX_HEADER_SIZE;
const MEM_POOL_SIZE: usize = 0x150;
const DATA_PTR_SIZE: usize = 8;

const START_OF_STR_SECTION: usize = HEADER_SIZE + MEM_POOL_SIZE + DATA_PTR_SIZE;

const STR_HEADER_SIZE: usize = 0x14;
const EMPTY_STR_SIZE: usize = 4;

const FILENAME_STR_OFFSET: usize = START_OF_STR_SECTION + STR_HEADER_SIZE + EMPTY_STR_SIZE;

const BRTD_SECTION_START: usize = 0xFF0;
const SIZE_OF_BRTD: usize = 0x10;
const START_OF_TEXTURE_DATA: usize = BRTD_SECTION_START + SIZE_OF_BRTD;

#[derive(BinRead, Debug)]
pub struct BntxFile {
    header: BntxHeader,

    #[br(is_little = header.bom == ByteOrder::LittleEndian)]
    nx_header: NxHeader,
}

impl BntxFile {
    pub fn width(&self) -> u32 {
        self.nx_header.info_ptr.width
    }

    pub fn height(&self) -> u32 {
        self.nx_header.info_ptr.height
    }

    pub fn depth(&self) -> u32 {
        self.nx_header.info_ptr.depth
    }

    pub fn num_array_layers(&self) -> u32 {
        self.nx_header.info_ptr.layer_count
    }

    pub fn num_mipmaps(&self) -> u32 {
        self.nx_header.info_ptr.mipmap_count as u32
    }

    pub fn image_format(&self) -> SurfaceFormat {
        self.nx_header.info_ptr.format
    }

    /// The deswizzled image data for all layers and mipmaps.
    pub fn deswizzled_data(&self) -> Result<Vec<u8>, tegra_swizzle::SwizzleError> {
        let info = &self.nx_header.info_ptr;

        deswizzle_surface(
            info.width as usize,
            info.height as usize,
            info.depth as usize,
            &info.texture.image_data,
            info.format.block_dim(),
            Some(BlockHeight::new(2u32.pow(info.block_height_log2) as usize).unwrap()),
            info.format.bytes_per_pixel(),
            info.mipmap_count as usize,
            info.layer_count as usize,
        )
    }

    pub fn write<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
    ) -> Result<(), binrw::error::Error> {
        let endian = binrw::Endian::Little;
        self.header.write_options(writer, endian, self)?;
        self.nx_header.write_options(writer, endian, self)?;

        (
            // memory pool
            &[0u8; 0x150][..],
            (START_OF_STR_SECTION
                + self.header.inner.str_section.get_size()
                + self.nx_header.dict.get_size()) as u64,
            &self.header.inner.str_section,
            &self.nx_header.dict,
        )
            .write_options(writer, endian, ())?;

        self.nx_header
            .info_ptr
            .write_options(writer, endian, self)?;

        vec![0u8; 512].write_options(writer, endian, ())?;

        for offset in &self.nx_header.info_ptr.texture.mipmap_offsets {
            offset.write_options(writer, endian, ())?;
        }
        let mipmaps_offset = writer.stream_position()?;

        let padding_size = BRTD_SECTION_START as u64 - mipmaps_offset;
        vec![0u8; padding_size as usize].write_options(writer, endian, ())?;

        // BRTD
        (
            b"BRTD",
            0,
            self.nx_header.info_ptr.texture.image_data.len() as u64 + 0x10,
        )
            .write_options(writer, endian, ())?;

        writer.write_all(&self.nx_header.info_ptr.texture.image_data)?;

        self.header
            .inner
            .reloc_table
            .write_options(writer, endian, ())?;

        Ok(())
    }

    pub fn from_image(
        img: image::DynamicImage,
        name: &str,
    ) -> Result<Self, tegra_swizzle::SwizzleError> {
        let data = img.to_rgba8().into_raw();

        Self::from_image_data(
            name,
            img.width(),
            img.height(),
            1,
            1,
            1,
            SurfaceFormat::R8G8B8A8Srgb,
            &data,
        )
    }

    /// Create a [BntxFile] from unswizzled image data.
    pub fn from_image_data(
        name: &str,
        width: u32,
        height: u32,
        depth: u32,
        mipmap_count: u32,
        layer_count: u32,
        format: SurfaceFormat,
        data: &[u8],
    ) -> Result<Self, tegra_swizzle::SwizzleError> {
        // Let tegra_swizzle calculate the block height.
        // This matches the value inferred for missing block heights like in nutexb.
        let block_dim = format.block_dim();
        let block_height = block_height_mip0(div_round_up(height as usize, block_dim.height.get()));

        let block_height_log2 = match block_height {
            BlockHeight::One => 0,
            BlockHeight::Two => 1,
            BlockHeight::Four => 2,
            BlockHeight::Eight => 3,
            BlockHeight::Sixteen => 4,
            BlockHeight::ThirtyTwo => 5,
        };

        let bytes_per_pixel = format.bytes_per_pixel();

        let data = swizzle_surface(
            width as usize,
            height as usize,
            depth as usize,
            data,
            block_dim,
            Some(block_height),
            bytes_per_pixel,
            mipmap_count as usize,
            layer_count as usize,
        )?;

        let str_section = StrSection {
            block_size: 0x58,
            block_offset: 0x58,
            strings: vec![BntxStr::from(name.to_owned())],
        };

        let str_section_size = str_section.get_size();
        let dict_section_size = (DictSection {
            node_count: 0,
            nodes: vec![],
        })
        .get_size();

        let mipmap_offsets = calculate_mipmap_offsets(
            mipmap_count,
            width,
            block_dim,
            height,
            depth,
            block_height,
            bytes_per_pixel,
        );

        Ok(Self {
            header: BntxHeader {
                version: (0, 4),
                bom: ByteOrder::LittleEndian,
                inner: HeaderInner {
                    revision: 0x400c,
                    file_name: name.into(),
                    str_section,
                    reloc_table: RelocationTable {
                        sections: vec![
                            RelocationSection {
                                pointer: 0,
                                position: 0,
                                size: (START_OF_STR_SECTION
                                    + str_section_size
                                    + dict_section_size
                                    + SIZE_OF_BRTI
                                    + 0x208) as u32,
                                index: 0,
                                count: 4,
                            },
                            RelocationSection {
                                pointer: 0,
                                position: BRTD_SECTION_START as u32,
                                size: (data.len() + SIZE_OF_BRTD) as u32,
                                index: 4,
                                count: 1,
                            },
                        ],
                        entries: vec![
                            RelocationEntry {
                                position: BNTX_HEADER_SIZE as u32 + 8,
                                struct_count: 2,
                                offset_count: 1,
                                padding_count: (((HEADER_SIZE + MEM_POOL_SIZE)
                                    - (BNTX_HEADER_SIZE + 0x10))
                                    / 8) as u8,
                            },
                            RelocationEntry {
                                position: BNTX_HEADER_SIZE as u32 + 0x18,
                                struct_count: 2,
                                offset_count: 2,
                                padding_count: ((START_OF_STR_SECTION
                                    + str_section_size
                                    + dict_section_size
                                    + 0x80
                                    - HEADER_SIZE)
                                    / 8) as u8,
                            },
                            RelocationEntry {
                                position: (START_OF_STR_SECTION + str_section_size + 0x10) as u32,
                                struct_count: 2,
                                offset_count: 1,
                                padding_count: 1,
                            },
                            RelocationEntry {
                                position: (START_OF_STR_SECTION
                                    + str_section_size
                                    + dict_section_size
                                    + 0x60) as u32,
                                struct_count: 1,
                                offset_count: 3,
                                padding_count: 0,
                            },
                            RelocationEntry {
                                position: (BNTX_HEADER_SIZE + 0x10) as u32,
                                struct_count: 2,
                                offset_count: 1,
                                padding_count: (((START_OF_STR_SECTION
                                    + str_section_size
                                    + dict_section_size
                                    + SIZE_OF_BRTI
                                    + 0x200)
                                    - (BNTX_HEADER_SIZE + 0x18))
                                    / 8) as u8,
                            },
                        ],
                    },
                },
            },
            nx_header: NxHeader {
                dict: DictSection {
                    node_count: 0,
                    nodes: vec![],
                },
                dict_size: 0x58,
                info_ptr: BrtiSection {
                    size: 3576,
                    size2: 3576,
                    flags: 1,
                    texture_dimension: TextureDimension::D2,
                    tile_mode: 0,
                    swizzle: 0,
                    mipmap_count: mipmap_count as u16,
                    multi_sample_count: 1,
                    format,
                    unk2: 32,
                    width,
                    height,
                    depth,
                    layer_count,
                    block_height_log2,
                    unk4: [65543, 0, 0, 0, 0, 0],
                    image_size: data.len() as _,
                    align: 512,
                    comp_sel: 84148994,
                    texture_view_dimension: TextureViewDimension::D2,
                    name_addr: name.to_owned().into(),
                    parent_addr: 32,
                    texture: Texture {
                        mipmap_offsets,
                        image_data: data,
                    },
                },
            },
        })
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, binrw::error::Error> {
        let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
        reader.read_le()
    }

    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), binrw::error::Error> {
        let mut writer = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        self.write(&mut writer)
    }
}

fn calculate_mipmap_offsets(
    mipmap_count: u32,
    width: u32,
    block_dim: BlockDim,
    height: u32,
    depth: u32,
    block_height: BlockHeight,
    bytes_per_pixel: usize,
) -> Vec<u64> {
    let mut mipmap_offsets = Vec::new();

    let mut mipmap_offset = 0;
    for mip in 0..mipmap_count {
        mipmap_offsets.push(START_OF_TEXTURE_DATA as u64 + mipmap_offset as u64);

        let mip_width = div_round_up((width as usize >> mip).max(1), block_dim.width.get());
        let mip_height = div_round_up((height as usize >> mip).max(1), block_dim.height.get());
        let mip_depth = div_round_up((depth as usize >> mip).max(1), block_dim.depth.get());
        let mip_block_height = mip_block_height(mip_height, block_height);
        let mip_size = tegra_swizzle::swizzle::swizzled_mip_size(
            mip_width,
            mip_height,
            mip_depth,
            mip_block_height,
            bytes_per_pixel,
        );

        mipmap_offset += mip_size;
    }
    mipmap_offsets
}

#[derive(BinRead, PartialEq, Debug, Clone, Copy)]
enum ByteOrder {
    #[br(magic = 0xFFFEu16)]
    LittleEndian,
    #[br(magic = 0xFEFFu16)]
    BigEndian,
}

#[derive(BinRead, Debug)]
#[br(magic = b"BNTX")]
struct BntxHeader {
    #[br(pad_before = 4)]
    version: (u16, u16),

    #[br(big)]
    bom: ByteOrder,

    #[br(is_little = bom == ByteOrder::LittleEndian)]
    inner: HeaderInner,
}

impl BntxHeader {
    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: binrw::Endian,
        parent: &BntxFile,
    ) -> Result<(), binrw::error::Error> {
        let start_of_reloc_section =
            (START_OF_TEXTURE_DATA + parent.nx_header.info_ptr.texture.image_data.len()) as u32;
        (
            b"BNTX",
            0u32,
            self.version,
            match self.bom {
                ByteOrder::LittleEndian => b"\xFF\xFE",
                ByteOrder::BigEndian => b"\xFE\xFF",
            },
            self.inner.revision,
            FILENAME_STR_OFFSET as u32 + 2,
            0u16,
            START_OF_STR_SECTION as u16,
            start_of_reloc_section,
            start_of_reloc_section + (self.inner.reloc_table.get_size() as u32),
        )
            .write_options(writer, options, ())
    }
}

#[binread]
#[derive(Debug)]
struct HeaderInner {
    revision: u16,

    #[br(parse_with = read_string_pointer)]
    file_name: String,

    #[br(pad_before = 2, parse_with = FilePtr16::parse)]
    str_section: StrSection,

    #[br(parse_with = FilePtr32::parse)]
    reloc_table: RelocationTable,

    #[br(temp)]
    file_size: u32,
}

fn read_string_pointer<'a, R: Read + Seek>(
    reader: &mut R,
    endian: binrw::Endian,
    args: <FilePtr32<NullString> as BinRead>::Args<'a>,
) -> BinResult<String> {
    FilePtr32::<NullString>::parse(reader, endian, args).map(|s| s.to_string())
}

#[derive(BinRead, BinWrite, Debug)]
struct RelocationSection {
    pointer: u64,
    position: u32,
    size: u32,
    index: u32,
    count: u32,
}

const SIZE_OF_RELOC_SECTION: usize = size_of::<u64>() + (size_of::<u32>() * 4);

#[derive(BinRead, BinWrite, Debug)]
struct RelocationEntry {
    position: u32,
    struct_count: u16,
    offset_count: u8,
    padding_count: u8,
}

const SIZE_OF_RELOC_ENTRY: usize = size_of::<u32>() + size_of::<u16>() + (size_of::<u8>() * 2);

#[binrw]
#[derive(Debug)]
#[brw(magic = b"_RLT")]
#[bw(stream = w)]
struct RelocationTable {
    #[br(temp)]
    #[bw(calc = w.stream_position().unwrap() as u32 - 4)]
    rlt_section_pos: u32,

    #[br(temp)]
    #[bw(calc = sections.len() as u32)]
    count: u32,

    #[br(pad_before = 4, count = count)]
    #[bw(pad_before = 4)]
    sections: Vec<RelocationSection>,

    #[br(count = sections.iter().map(|x| x.count).sum::<u32>())]
    entries: Vec<RelocationEntry>,
}

use core::mem::size_of;

impl RelocationTable {
    fn get_size(&self) -> usize {
        b"_RLT".len()
            + size_of::<u32>()
            + size_of::<u32>()
            + size_of::<u32>()
            + (self.sections.len() * SIZE_OF_RELOC_SECTION)
            + (self.entries.len() * SIZE_OF_RELOC_ENTRY)
    }
}

#[binrw]
#[derive(Debug)]
#[brw(magic = b"_STR")]
struct StrSection {
    block_size: u32,
    block_offset: u64,

    #[br(temp)]
    #[bw(calc = strings.len() as u32)]
    str_count: u32,

    #[br(temp)]
    #[bw(calc = BntxStr::from(String::new()))]
    empty: BntxStr,

    #[br(count = str_count)]
    #[bw(align_after = 8)]
    strings: Vec<BntxStr>,
}

impl StrSection {
    fn get_size(&self) -> usize {
        align(
            (5 * size_of::<u32>())
                + EMPTY_STR_SIZE
                + self.strings.iter().map(|x| x.get_size()).sum::<usize>(),
            8,
        )
    }
}

#[binrw]
#[derive(Debug)]
struct BntxStr {
    #[br(temp)]
    #[bw(calc = chars.len() as u16)]
    len: u16,

    #[br(align_after = 4, count = len, map = |x: Vec<u8>| String::from_utf8_lossy(&x).into_owned())]
    #[bw(align_after = 4, map = |s| bytes_null_terminated(s))]
    chars: String,
}

fn bytes_null_terminated(s: &str) -> Vec<u8> {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0u8);
    bytes
}

fn align(x: usize, n: usize) -> usize {
    (x + n - 1) & !(n - 1)
}

impl BntxStr {
    fn get_size(&self) -> usize {
        align(size_of::<u16>() + self.chars.bytes().len() + 1, 4)
    }
}

impl From<String> for BntxStr {
    fn from(chars: String) -> Self {
        BntxStr { chars }
    }
}

impl From<BntxStr> for String {
    fn from(bntx_str: BntxStr) -> String {
        bntx_str.chars
    }
}

// TODO: Rework this to write everything in a single pass.
// TODO: is there a simple algorithm to calculate the absolute offsets?
#[binread]
#[derive(Debug)]
#[br(magic = b"NX  ")]
struct NxHeader {
    #[br(temp)]
    count: u32,

    #[br(parse_with = read_double_indirect)]
    info_ptr: BrtiSection,

    #[br(temp)]
    data_blk_ptr: u64, // BRTD pointer

    #[br(parse_with = FilePtr64::parse)]
    dict: DictSection,
    dict_size: u64, // TODO: How to calculate this
                    // 136 bytes of padding
}

impl NxHeader {
    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: binrw::Endian,
        parent: &BntxFile,
    ) -> Result<(), binrw::error::Error> {
        (
            b"NX  ",
            1u32, // count
            (HEADER_SIZE + MEM_POOL_SIZE) as u64,
            BRTD_SECTION_START as u64,
            (START_OF_STR_SECTION + parent.header.inner.str_section.get_size()) as u64,
            self.dict_size,
        )
            .write_options(writer, options, ())
    }
}

#[derive(BinRead, Debug)]
#[br(magic = b"_DIC")]
struct DictSection {
    node_count: u32,
    // TODO: some sort of root node is always included?
    #[br(count = node_count + 1)]
    nodes: Vec<DictNode>,
}

#[derive(Debug, BinRead)]
struct DictNode {
    reference: i32,
    left_index: u16,
    right_index: u16,
    #[br(parse_with = FilePtr64::parse)]
    name: BntxStr,
}

// TODO: Derive binwrite instead.
static DICT_SECTION: &[u8] = b"\x5F\x44\x49\x43\x01\x00\x00\x00\xFF\xFF\xFF\xFF\x01\x00\x00\x00\xB4\x01\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x01\x00\xB8\x01\x00\x00\x00\x00\x00\x00";

impl DictSection {
    fn get_size(&self) -> usize {
        DICT_SECTION.len()
    }
}

impl BinWrite for DictSection {
    type Args<'a> = ();

    fn write_options<W: io::Write + Seek>(
        &self,
        writer: &mut W,
        endian: binrw::Endian,
        args: Self::Args<'_>,
    ) -> BinResult<()> {
        DICT_SECTION.write_options(writer, endian, args)
    }
}

// TODO: Are these flags?
#[binrw]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[brw(repr(u32))]
pub enum SurfaceFormat {
    R8Unorm = 0x0201,
    R8G8B8A8Unorm = 0x0b01,
    R8G8B8A8Srgb = 0x0b06,
    B8G8R8A8Unorm = 0x0c01,
    B8G8R8A8Srgb = 0x0c06,
    BC1Unorm = 0x1a01,
    BC1Srgb = 0x1a06,
    BC2Unorm = 0x1b01,
    BC2Srgb = 0x1b06,
    BC3Unorm = 0x1c01,
    BC3Srgb = 0x1c06,
    BC4Unorm = 0x1d01,
    BC4Snorm = 0x1d02,
    BC5Unorm = 0x1e01,
    BC5Snorm = 0x1e02,
    BC6Sfloat = 0x1f05,
    BC6Ufloat = 0x1f0a,
    BC7Unorm = 0x2001,
    BC7Srgb = 0x2006,
    // TODO: Fill in other known formats.
}

impl SurfaceFormat {
    fn bytes_per_pixel(&self) -> usize {
        match self {
            SurfaceFormat::R8Unorm => 1,
            SurfaceFormat::R8G8B8A8Unorm => 4,
            SurfaceFormat::R8G8B8A8Srgb => 4,
            SurfaceFormat::B8G8R8A8Unorm => 4,
            SurfaceFormat::B8G8R8A8Srgb => 4,
            SurfaceFormat::BC1Unorm => 8,
            SurfaceFormat::BC1Srgb => 8,
            SurfaceFormat::BC2Unorm => 16,
            SurfaceFormat::BC2Srgb => 16,
            SurfaceFormat::BC3Unorm => 16,
            SurfaceFormat::BC3Srgb => 16,
            SurfaceFormat::BC4Unorm => 8,
            SurfaceFormat::BC4Snorm => 8,
            SurfaceFormat::BC5Unorm => 16,
            SurfaceFormat::BC5Snorm => 16,
            SurfaceFormat::BC6Sfloat => 16,
            SurfaceFormat::BC6Ufloat => 16,
            SurfaceFormat::BC7Unorm => 16,
            SurfaceFormat::BC7Srgb => 16,
        }
    }

    fn block_dim(&self) -> BlockDim {
        match self {
            SurfaceFormat::R8Unorm => BlockDim::uncompressed(),
            SurfaceFormat::R8G8B8A8Unorm => BlockDim::uncompressed(),
            SurfaceFormat::R8G8B8A8Srgb => BlockDim::uncompressed(),
            SurfaceFormat::B8G8R8A8Unorm => BlockDim::uncompressed(),
            SurfaceFormat::B8G8R8A8Srgb => BlockDim::uncompressed(),
            SurfaceFormat::BC1Unorm => BlockDim::block_4x4(),
            SurfaceFormat::BC1Srgb => BlockDim::block_4x4(),
            SurfaceFormat::BC2Unorm => BlockDim::block_4x4(),
            SurfaceFormat::BC2Srgb => BlockDim::block_4x4(),
            SurfaceFormat::BC3Unorm => BlockDim::block_4x4(),
            SurfaceFormat::BC3Srgb => BlockDim::block_4x4(),
            SurfaceFormat::BC4Unorm => BlockDim::block_4x4(),
            SurfaceFormat::BC4Snorm => BlockDim::block_4x4(),
            SurfaceFormat::BC5Unorm => BlockDim::block_4x4(),
            SurfaceFormat::BC5Snorm => BlockDim::block_4x4(),
            SurfaceFormat::BC6Sfloat => BlockDim::block_4x4(),
            SurfaceFormat::BC6Ufloat => BlockDim::block_4x4(),
            SurfaceFormat::BC7Unorm => BlockDim::block_4x4(),
            SurfaceFormat::BC7Srgb => BlockDim::block_4x4(),
        }
    }
}

#[derive(BinRead, Debug)]
#[br(magic = b"BRTI")]
struct BrtiSection {
    size: u32,  // offset?
    size2: u64, // size?
    flags: u8,
    texture_dimension: TextureDimension,
    tile_mode: u16,
    swizzle: u16,
    mipmap_count: u16,
    multi_sample_count: u32,
    format: SurfaceFormat,
    unk2: u32,
    width: u32,
    height: u32,
    depth: u32,
    layer_count: u32,
    block_height_log2: u32,
    unk4: [u32; 6],  // TODO: What is this?
    image_size: u32, // the total size of all layers and mipmaps with padding
    align: u32,      // usually 512 to match the expected mipmap alignment for swizzled surfaces.
    comp_sel: u32,
    texture_view_dimension: TextureViewDimension,

    #[br(parse_with = FilePtr64::parse)]
    name_addr: BntxStr, // u64 pointer to name
    parent_addr: u64, // pointer to nx header

    // TODO: This is a pointer to an array of u64 mipmap offsets.
    // TODO: Parse the entire surface in one vec but store the mipmap offsets?
    #[br(parse_with = FilePtr64::parse, args { offset: 0, inner: (image_size, mipmap_count)} )]
    texture: Texture,
    // TODO: Additional fields?
}

#[binrw]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[brw(repr(u8))]
pub enum TextureDimension {
    D1 = 1,
    D2 = 2,
    D3 = 3,
}

#[binrw]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[brw(repr(u32))]
pub enum TextureViewDimension {
    D1 = 0,
    D2 = 1,
    D3 = 2,
    Cube = 3,
    // TODO: Fill in other known variants
}

const SIZE_OF_BRTI: usize = 0xA0;

impl BrtiSection {
    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        endian: binrw::Endian,
        parent: &BntxFile,
    ) -> Result<(), binrw::error::Error> {
        (
            (
                b"BRTI",
                self.size,
                self.size2,
                self.flags,
                self.texture_dimension,
                self.tile_mode,
                self.swizzle,
                self.mipmap_count,
                self.multi_sample_count,
                &self.format,
                self.unk2,
                self.width,
                self.height,
                self.depth,
                self.layer_count,
                self.block_height_log2,
                self.unk4,
                self.image_size,
                self.align,
                self.comp_sel,
            ),
            self.texture_view_dimension,
            FILENAME_STR_OFFSET as u64,
            BNTX_HEADER_SIZE as u64,
            (START_OF_STR_SECTION
                + parent.header.inner.str_section.get_size()
                + parent.nx_header.dict.get_size()
                + SIZE_OF_BRTI
                + 0x200) as u64,
            0u64,
            (START_OF_STR_SECTION
                + parent.header.inner.str_section.get_size()
                + parent.nx_header.dict.get_size()
                + SIZE_OF_BRTI) as u64,
            (START_OF_STR_SECTION
                + parent.header.inner.str_section.get_size()
                + parent.nx_header.dict.get_size()
                + SIZE_OF_BRTI
                + 0x100) as u64,
            0u64,
            0u64,
        )
            .write_options(writer, endian, ())
    }
}

use binrw::io::{Read, Seek};

fn read_double_indirect<'a, T: BinRead, R: Read + Seek>(
    reader: &mut R,
    endian: binrw::Endian,
    args: T::Args<'a>,
) -> BinResult<T> {
    let offset1 = <u64>::read_options(reader, endian, ())?;
    let position = reader.stream_position()?;

    reader.seek(SeekFrom::Start(offset1))?;
    let offset2 = <u64>::read_options(reader, endian, ())?;

    reader.seek(SeekFrom::Start(offset2))?;
    let value = T::read_options(reader, endian, args)?;

    reader.seek(SeekFrom::Start(position))?;
    Ok(value)
}

#[derive(BinRead)]
#[br(import(image_size: u32, mipmap_count: u16))]
struct Texture {
    #[br(count = mipmap_count)]
    mipmap_offsets: Vec<u64>,

    // TODO: Handle the case where the mipmaps are empty.
    // TODO: Just write a custom parse function?
    #[br(count = image_size, seek_before = SeekFrom::Start(mipmap_offsets[0]))]
    image_data: Vec<u8>,
}

impl fmt::Debug for Texture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImageData[{:?}]", self.mipmap_offsets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dds::create_bntx;
    use crate::dds::create_dds;
    use std::io::BufWriter;

    #[test]
    fn try_parse() {
        let original = BntxFile::from_file("chara_1_mario_00.bntx").unwrap();

        original.write_to_file("chara_1_mario_00.out.bntx").unwrap();

        let dds = create_dds(&original).unwrap();
        let mut writer = BufWriter::new(std::fs::File::create("chara_1_mario_00.dds").unwrap());
        dds.write(&mut writer).unwrap();

        create_bntx("chara_1_mario_00", &dds)
            .unwrap()
            .write_to_file("chara_1_mario_00.dds.bntx")
            .unwrap();
    }
}
