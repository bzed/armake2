//! Functions for working with PAA files.
//!
//! Format (as used by DayZ / Arma 3, verified against engine-loaded files):
//!
//! ```text
//! u16  magic (compression: 0xff01 DXT1, 0xff02 DXT2, 0xff03 DXT3,
//!             0xff04 DXT4, 0xff05 DXT5, 0x8888 RGBA, ...)
//! taggs: 8-byte signature (e.g. "GGATSFFO") + u32 data size + data,
//!        repeated until a zero byte (which is the low byte of the
//!        palette length following them)
//! u16  palette size in bytes (+ palette data if non-zero)
//! mipmaps: u16 width (bit 0x8000 = LZO compressed) + u16 height +
//!          3-byte (little endian) data size + data, repeated until a
//!          zero width
//! u16 x3 zero terminator
//! ```

use std::fs::File;
use std::io::{Read, Write, Seek, SeekFrom, Error};
use std::path::PathBuf;

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use image::{DynamicImage, ImageBuffer, Rgba};
use linked_hash_map::LinkedHashMap;
use texpresso::{Algorithm, Format, Params};
use lzo1x;
use crate::error::*;

const OFFS_TAGG: &str = "GGATSFFO"; // mipmap offsets (16 u32, zero padded)

#[derive(Debug)]
pub struct Paa {
    pub taggs: LinkedHashMap<String, Vec<u8>>,
    pub palette: Option<Vec<u8>>,
    pub mipmaps: Vec<Vec<u8>>,
    pub width: u16,
    pub height: u16,
    pub compression: String,
}

impl Paa {
    pub fn read<I: Read + Seek>(input: &mut I) -> Result<Paa, Error> {
        // The top-level read function. We seek to the beginning to ensure consistent state,
        // as this might be called on a stream that has been partially read (e.g. by another
        // part of the program trying to determine file type).
        input.seek(SeekFrom::Start(0))?;

        let magic_number = input.read_u16::<LittleEndian>()?;
        let compression = match magic_number {
            0xff01 => "DXT1",
            0xff02 => "DXT2",
            0xff03 => "DXT3",
            0xff04 => "DXT4",
            0xff05 => "DXT5",
            0x8888 => "RGBA",
            0x4444 => return Err(error!("Unsupported PAA format: RGBA4444 (0x4444)")),
            0x1555 => return Err(error!("Unsupported PAA format: RGBA5551 (0x1555)")),
            0x8080 => return Err(error!("Unsupported PAA format: GRAYwAlpha (0x8080)")),
            _ => return Err(error!("Invalid PAA magic number: 0x{:04x}", magic_number)),
        }.to_string();

        let mut taggs = LinkedHashMap::new();
        let mut mipmaps = Vec::new();
        let mut width = 0;
        let mut height = 0;

        // Taggs. The section is terminated by a zero byte, which is not
        // consumed: it is the low byte of the palette size that follows.
        loop {
            let mut next_byte = [0u8; 1];
            if input.read(&mut next_byte)? == 0 { break; }
            if next_byte[0] == 0 {
                input.seek(SeekFrom::Current(-1))?;
                break;
            }
            input.seek(SeekFrom::Current(-1))?;

            let mut tagg_name_bytes = [0u8; 8];
            input.read_exact(&mut tagg_name_bytes)?;
            let tagg_name = String::from_utf8_lossy(&tagg_name_bytes).trim_end_matches('\0').to_string();

            let tagg_size = input.read_u32::<LittleEndian>()? as usize;
            let mut data = vec![0; tagg_size];
            input.read_exact(&mut data)?;

            taggs.insert(tagg_name, data);
        }

        let palette_len = input.read_u16::<LittleEndian>()? as usize;
        let palette = if palette_len > 0 {
            let mut pal_data = vec![0; palette_len];
            input.read_exact(&mut pal_data)?;
            Some(pal_data)
        } else {
            None
        };

        // Mipmaps
        loop {
            let w = match input.read_u16::<LittleEndian>() {
                Ok(0) | Err(_) => break,
                Ok(w) => w,
            };
            let h = input.read_u16::<LittleEndian>()?;

            let mut size_bytes = [0u8; 3];
            input.read_exact(&mut size_bytes)?;
            let size = (size_bytes[0] as usize) | ((size_bytes[1] as usize) << 8) | ((size_bytes[2] as usize) << 16);

            if width == 0 { width = w & 0x7FFF; }
            if height == 0 { height = h; }

            let mut mip_data = vec![0; size];
            input.read_exact(&mut mip_data)?;

            if (w & 0x8000) != 0 {
                // LZO compressed. The decompressed size is the format's
                // usual compressed (block) size for the mipmap dimensions.
                let real_w = (w & 0x7FFF) as usize;
                let real_h = h as usize;
                let decompressed_size = match compression.as_str() {
                    "DXT1" => Format::Bc1.compressed_size(real_w, real_h),
                    "DXT2" | "DXT3" => Format::Bc2.compressed_size(real_w, real_h),
                    "DXT4" | "DXT5" => Format::Bc3.compressed_size(real_w, real_h),
                    _ => return Err(error!("LZO compression is only supported for DXT formats, not '{}'", compression)),
                };
                let mut decompressed_data = vec![0; decompressed_size];
                lzo1x::decompress(&mip_data, &mut decompressed_data)
                    .map_err(|e| error!("Failed to decompress LZO-compressed mipmap: {:?}", e))?;
                mipmaps.push(decompressed_data);
            } else {
                mipmaps.push(mip_data);
            }
        }

        if mipmaps.is_empty() {
            return Err(error!("PAA contains no mipmap data."));
        }

        Ok(Paa {
            taggs,
            palette,
            mipmaps,
            width,
            height,
            compression,
        })
    }

    pub fn to_dynamic_image(&self) -> Result<DynamicImage, Error> {
        if self.mipmaps.is_empty() {
            return Err(error!("PAA has no image data"));
        }

        let main_mipmap = &self.mipmaps[0];
        let width = self.width as usize;
        let height = self.height as usize;

        let format = match self.compression.as_str() {
            "DXT1" => Format::Bc1,
            "DXT2" => Format::Bc2,
            "DXT3" => Format::Bc2,
            "DXT4" => Format::Bc3,
            "DXT5" => Format::Bc3,
            "PAL" => {
                let palette = self.palette.as_ref().ok_or_else(|| error!("Paletted PAA without palette data"))?;
                let mut rgba_data = Vec::with_capacity(width * height * 4);
                for &index in main_mipmap {
                    let i = index as usize * 4;
                    if i + 3 >= palette.len() {
                        return Err(error!("Palette index out of bounds"));
                    }
                    // Palette is BGRA
                    rgba_data.push(palette[i + 2]); // R
                    rgba_data.push(palette[i + 1]); // G
                    rgba_data.push(palette[i]); // B
                    rgba_data.push(palette[i + 3]); // A
                }
                let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width as u32, height as u32, rgba_data).unwrap();
                return Ok(DynamicImage::ImageRgba8(img));
            }
            "RGBA" => {
                if main_mipmap.len() == width * height * 4 {
                    let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width as u32, height as u32, main_mipmap.clone()).unwrap();
                    return Ok(DynamicImage::ImageRgba8(img));
                }
                return Err(error!("Invalid uncompressed RGBA data size"));
            }
            _ => return Err(error!("Unsupported PAA compression format: {}", self.compression)),
        };

        let mut decompressed = vec![0u8; width * height * 4];
        format.decompress(main_mipmap, width, height, &mut decompressed);

        let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width as u32, height as u32, decompressed).unwrap();
        Ok(DynamicImage::ImageRgba8(img))
    }

    pub fn from_dynamic_image(img: &DynamicImage, paa_type: &str, _compress: bool) -> Result<Paa, Error> {
        let mut current = img.to_rgba8();
        let compression = match paa_type.to_lowercase().as_str() {
            "dxt1" | "dxt3" | "dxt5" => paa_type.to_uppercase(),
            "rgba" => "RGBA".to_string(),
            _ => return Err(error!("Unsupported PAA compression type for creation: {}", paa_type)),
        };

        // Build the mipmap chain down to 4x4.
        let mut levels: Vec<ImageBuffer<Rgba<u8>, Vec<u8>>> = vec![current.clone()];
        loop {
            let (w, h) = current.dimensions();
            if w <= 4 || h <= 4 { break; }
            current = image::imageops::resize(&current, w / 2, h / 2, image::imageops::FilterType::Triangle);
            levels.push(current.clone());
        }

        let params = Params {
            algorithm: Algorithm::RangeFit,
            weights: [1.0, 1.0, 1.0],
            weigh_colour_by_alpha: false,
        };

        let mut mipmaps = Vec::new();
        for level in &levels {
            let (w, h) = level.dimensions();
            let data = level.as_raw().to_vec();
            let mut mipmap = match compression.as_str() {
                "DXT1" => {
                    let mut out = vec![0; Format::Bc1.compressed_size(w as usize, h as usize)];
                    Format::Bc1.compress(&data, w as usize, h as usize, params, &mut out);
                    out
                }
                "DXT3" => {
                    let mut out = vec![0; Format::Bc2.compressed_size(w as usize, h as usize)];
                    Format::Bc2.compress(&data, w as usize, h as usize, params, &mut out);
                    out
                }
                "DXT5" => {
                    let mut out = vec![0; Format::Bc3.compressed_size(w as usize, h as usize)];
                    Format::Bc3.compress(&data, w as usize, h as usize, params, &mut out);
                    out
                }
                "RGBA" => data,
                _ => unreachable!(),
            };
            // LZO compress large mipmaps like the engine's own tools do.
            if compression != "RGBA" && w > 128 {
                let compressed = lzo1x::compress(&mipmap, lzo1x::CompressLevel::default());
                if compressed.len() < mipmap.len() {
                    let mut compressed = compressed;
                    compressed.push(0); // LZO terminator
                    mipmap = compressed;
                }
            }
            mipmaps.push(mipmap);
        }

        let (width, height) = levels[0].dimensions();

        Ok(Paa {
            taggs: LinkedHashMap::new(),
            palette: None,
            mipmaps,
            width: width as u16,
            height: height as u16,
            compression,
        })
    }

    pub fn write<O: Write>(&self, output: &mut O) -> Result<(), Error> {
        let magic_number: u16 = match self.compression.as_str() {
            "DXT1" => 0xff01,
            "DXT2" => 0xff02,
            "DXT3" => 0xff03,
            "DXT4" => 0xff04,
            "DXT5" => 0xff05,
            "RGBA" => 0x8888,
            _ => return Err(error!("Cannot write PAA compression type '{}'.", self.compression)),
        };
        output.write_u16::<LittleEndian>(magic_number)?;

        for (name, data) in &self.taggs {
            let mut name_bytes = [0u8; 8];
            let name = name.as_bytes();
            if name.len() > 8 {
                return Err(error!("PAA tagg name '{}' is too long (max 8 bytes).", String::from_utf8_lossy(name)));
            }
            name_bytes[..name.len()].copy_from_slice(name);
            output.write_all(&name_bytes)?;
            output.write_u32::<LittleEndian>(data.len() as u32)?;
            output.write_all(data)?;
        }

        // Mipmap offset tagg (GGATSFFO): 16 u32 offsets, zero padded.
        let num_mipmaps = self.mipmaps.len().min(16);
        let mut offset = 2u32; // magic
        for (_, data) in &self.taggs {
            offset += 8 + 4 + data.len() as u32;
        }
        offset += 8 + 4 + 64; // offset tagg itself (padded to 16 u32)
        offset += 2; // palette size
        if let Some(palette) = &self.palette {
            offset += palette.len() as u32;
        }

        let mut offsets: Vec<u32> = Vec::new();
        let mut current_offset = offset;
        let mut current_width = self.width;
        let mut current_height = self.height;
        for mipmap in self.mipmaps.iter().take(num_mipmaps) {
            offsets.push(current_offset);
            current_offset += mipmap.len() as u32 + 2 + 2 + 3;
            if current_width > 1 { current_width /= 2; }
            if current_height > 1 { current_height /= 2; }
        }
        while offsets.len() < 16 {
            offsets.push(0);
        }

        output.write_all(OFFS_TAGG.as_bytes())?;
        output.write_u32::<LittleEndian>((16 * 4) as u32)?;
        for offset in offsets {
            output.write_u32::<LittleEndian>(offset)?;
        }

        // Palette size. The zero doubles as the tagg section terminator.
        output.write_u16::<LittleEndian>(self.palette.as_ref().map(|p| p.len()).unwrap_or(0) as u16)?;
        if let Some(palette) = &self.palette {
            output.write_all(palette)?;
        }

        // Mipmaps
        let mut current_width = self.width;
        let mut current_height = self.height;
        for mipmap in &self.mipmaps {
            let mut w = current_width;
            if self.compression != "RGBA" && current_width > 128 {
                w |= 0x8000; // LZO compressed
            }
            output.write_u16::<LittleEndian>(w)?;
            output.write_u16::<LittleEndian>(current_height)?;
            let len = mipmap.len() as u32;
            output.write_all(&[len as u8, (len >> 8) as u8, (len >> 16) as u8])?;
            output.write_all(mipmap)?;

            if current_width > 1 { current_width /= 2; }
            if current_height > 1 { current_height /= 2; }
        }

        // Terminator
        output.write_u16::<LittleEndian>(0)?;
        output.write_u16::<LittleEndian>(0)?;
        output.write_u16::<LittleEndian>(0)?;

        Ok(())
    }
}

pub fn cmd_paa2img(input_path: PathBuf, output_path: PathBuf, force: bool) -> Result<(), Error> {
    if !force && output_path.exists() {
        return Err(error!("Target file \"{}\" already exists. Use --force to overwrite.", output_path.display()));
    }

    let mut file = File::open(&input_path).prepend_error("Failed to open input PAA file:")?;
    let paa = Paa::read(&mut file).prepend_error("Failed to read PAA file:")?;
    let img = paa.to_dynamic_image().prepend_error("Failed to convert PAA to image:")?;

    img.save(&output_path).map_err(|e| error!("{}", e)).prepend_error("Failed to save output image:")?;

    Ok(())
}

pub fn cmd_img2paa(input_path: PathBuf, output_path: PathBuf, paa_type: String, compress: bool, force: bool) -> Result<(), Error> {
    if !force && output_path.exists() {
        return Err(error!("Target file \"{}\" already exists. Use --force to overwrite.", output_path.display()));
    }

    let img = image::open(&input_path).map_err(|e| error!("{}", e)).prepend_error("Failed to open input image file:")?;
    let paa = Paa::from_dynamic_image(&img, &paa_type, compress).prepend_error("Failed to convert image to PAA:")?;

    let mut file = File::create(&output_path).prepend_error("Failed to create output PAA file:")?;
    paa.write(&mut file).prepend_error("Failed to write PAA file:")?;

    Ok(())
}
