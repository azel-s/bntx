use binrw::binread;
use binrw::binrw;
use binrw::prelude::*;
use binrw::{BinWrite, WriteOptions};
use binrw::{FilePtr16, FilePtr32, FilePtr64, NullString};
use std::error::Error;
use std::path::Path;
use std::{fmt, io};
use tegra_swizzle::block_height_mip0;
use tegra_swizzle::div_round_up;
use tegra_swizzle::surface::{deswizzle_surface, swizzle_surface, BlockDim};
use tegra_swizzle::BlockHeight;

// TODO: Add module level docs for basic usage.
// TODO: Make this optional.
pub mod dds;

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

impl BntxHeader {
    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: &WriteOptions,
        parent: &BntxFile,
    ) -> Result<(), binrw::error::Error> {
        let start_of_reloc_section =
            (START_OF_TEXTURE_DATA + parent.nx_header.info_ptr.texture.0.len()) as u32;
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

    #[br(parse_with = FilePtr32::parse, map = |x: NullString| x.to_string())]
    file_name: String,

    #[br(pad_before = 2, parse_with = FilePtr16::parse)]
    str_section: StrSection,

    #[br(parse_with = FilePtr32::parse)]
    reloc_table: RelocationTable,

    #[br(temp)]
    file_size: u32,
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

#[binread]
#[derive(Debug)]
#[br(magic = b"_RLT")]
struct RelocationTable {
    #[br(temp)]
    rlt_section_pos: u32,

    #[br(temp)]
    count: u32,

    #[br(pad_before = 4, count = count)]
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

    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: &WriteOptions,
        parent: &BntxFile,
    ) -> Result<(), binrw::error::Error> {
        (
            b"_RLT",
            (START_OF_TEXTURE_DATA + parent.nx_header.info_ptr.texture.0.len()) as u32,
            self.sections.len() as u32,
            0u32,
            &self.sections,
            &self.entries,
        )
            .write_options(writer, options, ())
    }
}

#[binread]
#[derive(Debug)]
#[br(magic = b"_STR")]
struct StrSection {
    unk: u32,
    unk2: u32,
    unk3: u32,

    #[br(temp)]
    str_count: u32,

    #[br(temp)]
    empty: BntxStr,

    #[br(count = str_count)]
    strings: Vec<BntxStr>,
}

impl BinWrite for StrSection {
    type Args = ();

    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: &WriteOptions,
        args: Self::Args,
    ) -> Result<(), binrw::error::Error> {
        (
            b"_STR",
            self.unk,
            self.unk2,
            self.unk3,
            self.strings.len() as u32,
            BntxStr::from(String::new()),
            &self.strings,
        )
            .write_options(writer, options, ())
    }
}

impl StrSection {
    fn get_size(&self) -> usize {
        (5 * size_of::<u32>())
            + EMPTY_STR_SIZE
            + self.strings.iter().map(|x| x.get_size()).sum::<usize>()
    }
}

#[binread]
#[derive(Debug)]
struct BntxStr {
    len: u16,

    #[br(align_after = 4, count = len, map = |x: Vec<u8>| String::from_utf8_lossy(&x).into_owned())]
    chars: String,
}

// TODO: Find a way to derive this implementation.
impl BinWrite for BntxStr {
    type Args = ();

    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: &WriteOptions,
        args: Self::Args,
    ) -> Result<(), binrw::error::Error> {
        self.len.write_options(writer, options, ())?;
        self.chars.as_bytes().write_options(writer, options, ())?;
        0u8.write_options(writer, options, ())?;
        // Align to 4 bytes.
        let pos = writer.stream_position()?;
        let new_pos = align(pos as usize, 4);
        dbg!(pos, new_pos);
        for _ in 0..(new_pos - pos as usize) {
            0u8.write_options(writer, options, ())?;
        }
        Ok(())
    }
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
        BntxStr {
            len: chars.len() as u16,
            chars,
        }
    }
}

impl From<BntxStr> for String {
    fn from(bntx_str: BntxStr) -> String {
        bntx_str.chars
    }
}

#[binread]
#[derive(Debug)]
#[br(magic = b"NX  ")]
struct NxHeader {
    #[br(temp)]
    count: u32,

    #[br(parse_with = read_double_indirect)]
    info_ptr: BrtiSection,

    #[br(temp)]
    data_blk_ptr: u64,

    #[br(parse_with = FilePtr64::parse)]
    dict: DictSection,
    dict_size: u64,
}

impl NxHeader {
    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: &WriteOptions,
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
    // lol
}

static DICT_SECTION: &[u8] = b"\x5F\x44\x49\x43\x01\x00\x00\x00\xFF\xFF\xFF\xFF\x01\x00\x00\x00\xB4\x01\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x01\x00\xB8\x01\x00\x00\x00\x00\x00\x00";

impl DictSection {
    fn get_size(&self) -> usize {
        DICT_SECTION.len()
    }
}

impl BinWrite for DictSection {
    type Args = ();

    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: &WriteOptions,
        args: Self::Args,
    ) -> Result<(), binrw::error::Error> {
        DICT_SECTION.write_options(writer, options, ())
    }
}

#[binrw]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[brw(repr(u32))]
enum SurfaceFormat {
    R8G8B8A8Srgb = 0x0b06,
    BC7Unorm = 0x2001,
    // TODO: Fill in other known formats.
}

impl SurfaceFormat {
    fn bytes_per_pixel(&self) -> usize {
        match self {
            SurfaceFormat::R8G8B8A8Srgb => 4,
            SurfaceFormat::BC7Unorm => 16,
        }
    }

    fn block_dim(&self) -> BlockDim {
        match self {
            SurfaceFormat::R8G8B8A8Srgb => BlockDim::uncompressed(),
            SurfaceFormat::BC7Unorm => BlockDim::block_4x4(),
        }
    }
}

#[derive(BinRead, Debug)]
#[br(magic = b"BRTI")]
struct BrtiSection {
    size: u32,
    size2: u64,
    flags: u8,
    dim: u8,
    tile_mode: u16,
    swizzle: u16,
    mips_count: u16,
    num_multi_sample: u32,
    format: SurfaceFormat,
    unk2: u32,
    width: u32,
    height: u32,
    depth: u32,
    layer_count: u32,
    block_height_log2: u32,
    unk4: [u32; 6],
    image_size: u32,
    align: u32,
    comp_sel: u32,
    ty: u32,

    #[br(parse_with = FilePtr64::parse)]
    name_addr: BntxStr,
    parent_addr: u64,

    #[br(args(image_size), parse_with = read_double_indirect)]
    texture: ImageData,
}

const SIZE_OF_BRTI: usize = 0xA0;

impl BrtiSection {
    fn write_options<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
        options: &WriteOptions,
        parent: &BntxFile,
    ) -> Result<(), binrw::error::Error> {
        (
            (
                b"BRTI",
                self.size,
                self.size2,
                self.flags,
                self.dim,
                self.tile_mode,
                self.swizzle,
                self.mips_count,
                self.num_multi_sample,
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
            self.ty,
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
            .write_options(writer, options, ())
    }
}

use binrw::{
    io::{Read, Seek},
    ReadOptions,
};

fn read_double_indirect<T: BinRead, R: Read + Seek>(
    reader: &mut R,
    options: &ReadOptions,
    args: T::Args,
) -> BinResult<T>
where
    T::Args: Copy,
{
    let mut data = <FilePtr64<FilePtr64<T>> as BinRead>::read_options(reader, options, args)?;

    data.after_parse(reader, options, args)?;

    Ok(data.into_inner().into_inner())
}

#[derive(BinRead)]
#[br(import(len: u32))]
struct ImageData(#[br(count = len)] pub Vec<u8>);

impl fmt::Debug for ImageData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImageData[{}]", self.0.len())
    }
}

#[derive(BinRead, Debug)]
pub struct BntxFile {
    header: BntxHeader,

    #[br(is_little = header.bom == ByteOrder::LittleEndian)]
    nx_header: NxHeader,
}

// TODO: Add DDS support similar to nutexb.
impl BntxFile {
    pub fn to_image(&self) -> image::DynamicImage {
        let info = &self.nx_header.info_ptr;

        let data = self.deswizzled_data().unwrap();

        // TODO: Don't assume RGBA.
        // TODO: Error if not RGBA?
        let base_size = info.width as usize * info.height as usize * 4;

        image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(info.width, info.height, data[..base_size].to_owned())
                .unwrap(),
        )
    }

    /// The deswizzled image data for all layers and mipmaps.
    pub fn deswizzled_data(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let info = &self.nx_header.info_ptr;

        deswizzle_surface(
            info.width as usize,
            info.height as usize,
            info.depth as usize,
            &info.texture.0,
            info.format.block_dim(),
            Some(BlockHeight::new(2u32.pow(info.block_height_log2) as usize).unwrap()),
            info.format.bytes_per_pixel(),
            info.mips_count as usize,
            info.layer_count as usize,
        )
        .map_err(Into::into)
    }

    pub fn write<W: io::Write + io::Seek>(
        &self,
        writer: &mut W,
    ) -> Result<(), binrw::error::Error> {
        let options = binrw::WriteOptions::new(binrw::Endian::Little);
        self.header.write_options(writer, &options, self)?;
        self.nx_header.write_options(writer, &options, self)?;

        (
            // memory pool
            &[0u8; 0x150][..],
            (START_OF_STR_SECTION
                + self.header.inner.str_section.get_size()
                + self.nx_header.dict.get_size()) as u64,
            &self.header.inner.str_section,
            &self.nx_header.dict,
        )
            .write_options(writer, &options, ())?;

        self.nx_header
            .info_ptr
            .write_options(writer, &options, self)?;

        vec![0u8; 512].write_options(writer, &options, ())?;

        0x1000u64.write_options(writer, &options, ())?;

        let padding_size = BRTD_SECTION_START
            - (START_OF_STR_SECTION
                + self.header.inner.str_section.get_size()
                + self.nx_header.dict.get_size()
                + SIZE_OF_BRTI
                + 0x200
                + DATA_PTR_SIZE);

        dbg!(padding_size);
        vec![0u8; padding_size].write_options(writer, &options, ())?;

        // BRTD
        (
            b"BRTD",
            0,
            self.nx_header.info_ptr.texture.0.len() as u64 + 0x10,
        )
            .write_options(writer, &options, ())?;

        writer.write_all(&self.nx_header.info_ptr.texture.0)?;

        self.header
            .inner
            .reloc_table
            .write_options(writer, &options, self)?;

        Ok(())
    }

    pub fn from_image(img: image::DynamicImage, name: &str) -> Result<Self, Box<dyn Error>> {
        let img = img.to_rgba8();

        let (width, height) = img.dimensions();

        let data = img.into_raw();

        // TODO: This should fail if the format isn't RGBA8 already.
        Self::from_image_data(
            name,
            width,
            height,
            1,
            1,
            1,
            SurfaceFormat::R8G8B8A8Srgb,
            &data,
        )
    }

    /// Create a [BntxFile] from unswizzled image data.
    fn from_image_data(
        name: &str,
        width: u32,
        height: u32,
        depth: u32,
        mips_count: u32,
        layer_count: u32,
        format: SurfaceFormat,
        data: &[u8],
    ) -> Result<Self, Box<dyn Error>> {
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

        let data = swizzle_surface(
            width as usize,
            height as usize,
            depth as usize,
            &data,
            block_dim,
            Some(block_height),
            format.bytes_per_pixel(),
            mips_count as usize,
            layer_count as usize,
        )?;

        let str_section = StrSection {
            unk: 0x48,
            unk2: 0x48,
            unk3: 0,
            strings: vec![BntxStr::from(name.to_owned())],
        };

        let str_section_size = str_section.get_size();
        let dict_section_size = (DictSection {}).get_size();

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
                dict: DictSection {},
                dict_size: 0x58,
                info_ptr: BrtiSection {
                    size: 3592,
                    size2: 3592,
                    flags: 1,
                    dim: 2,
                    tile_mode: 0,
                    swizzle: 0,
                    mips_count: mips_count as u16,
                    num_multi_sample: 1,
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
                    ty: 1,
                    name_addr: name.to_owned().into(),
                    parent_addr: 32,
                    texture: ImageData(data),
                },
            },
        })
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), binrw::error::Error> {
        let mut file = std::fs::File::create(path.as_ref())?;

        self.write(&mut file)
    }
}

#[cfg(test)]
mod tests {
    use super::BntxFile;
    use crate::dds::create_bntx;
    use crate::dds::create_dds;
    use binrw::prelude::*;
    use std::io::BufReader;
    use std::io::BufWriter;

    #[test]
    fn try_parse() {
        let mut data = BufReader::new(std::fs::File::open("spirits_0_abra.bntx").unwrap());
        let test: BntxFile = data.read_le().unwrap();

        dbg!(&test);

        let mut writer = BufWriter::new(std::fs::File::create("spirits_0_abra.out.bntx").unwrap());
        test.write(&mut writer).unwrap();

        let dds = create_dds(&test).unwrap();
        let mut writer = BufWriter::new(std::fs::File::create("spirits_0_abra.dds").unwrap());
        dds.write(&mut writer).unwrap();

        let mut writer = BufWriter::new(std::fs::File::create("spirits_0_abra.dds.bntx").unwrap());
        create_bntx("spirts_0_abra", &dds)
            .unwrap()
            .write(&mut writer)
            .unwrap();
    }
}
