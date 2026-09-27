//! Reading from the first bytes what only a system decoder could read.
//!
//! This is the first of two gates in front of a system decoder. It runs on
//! every system and in front of any [`crate::PlatformDecoder`], a test's
//! substitute included, so a PDF, an archive, or plain text is refused here by
//! type and no decoder is ever asked about it. The second gate is the system
//! adapter's own check of what the system says the bytes are.

/// ISO media brands that name an image this crate's documentation lists: HEIC
/// and HEIF stills and sequences, AVIF stills and sequences, and Canon CR3.
const IMAGE_BRANDS: [&[u8; 4]; 13] = [
    b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs", b"mif1", b"msf1",
    b"avif", b"avis", b"crx ",
];
/// Compatible brands read from a `ftyp` box, after its major brand. A real file
/// lists a handful; this only stops a hostile box length from driving a scan.
const MAX_COMPATIBLE_BRANDS: usize = 32;
/// A RAW file's first directory holds a thumbnail no longer than this on its
/// long edge: cameras write 160 by 120 or thereabouts. A TIFF anyone would
/// build a resolution pyramid for is far larger.
const RAW_THUMBNAIL_MAX_EDGE_PX: u32 = 1_024;

/// Whether `input` begins like HEIC or HEIF, AVIF, JPEG XL, PSD, or a camera
/// RAW file that is not a TIFF container. A RAW file that is one is recognised
/// by [`tiff_holds_only_a_preview`] instead.
///
/// The list is explicit, so a RAW container missing from it is refused as
/// unsupported even where the system could read it: Fujifilm RAF, Panasonic
/// RW2, Olympus ORF, Canon CR3, and Sigma X3F are here.
pub(crate) fn names_a_system_encoding(input: &[u8]) -> bool {
    const JPEG_XL_CONTAINER: [u8; 12] = [0, 0, 0, 12, b'J', b'X', b'L', b' ', 13, 10, 0x87, 10];
    let starts = |magic: &[u8]| input.starts_with(magic);
    iso_media_image(input)
        || starts(&[0xff, 0x0a]) // a bare JPEG XL codestream
        || starts(&JPEG_XL_CONTAINER)
        || starts(b"8BPS") // PSD
        || starts(b"FUJIFILMCCD-RAW") // RAF
        || starts(b"IIU\0") // RW2
        || starts(b"IIRO") // ORF
        || starts(b"IIRS")
        || starts(b"MMOR")
        || starts(b"FOVb") // X3F
}

/// Whether `input` opens with an ISO media `ftyp` box whose major brand, or any
/// compatible brand inside the box, is one of [`IMAGE_BRANDS`]. An MP4 video or
/// an audio file is the same container with other brands, and is not an image.
fn iso_media_image(input: &[u8]) -> bool {
    if input.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let declared = u32::from_be_bytes([input[0], input[1], input[2], input[3]]) as usize;
    let end = declared.min(input.len());
    let is_image = |brand: &[u8]| IMAGE_BRANDS.iter().any(|known| brand == *known);
    // The major brand, four bytes of version, then the compatible brands.
    input.get(8..12).is_some_and(is_image)
        || input.get(16..end).is_some_and(|brands| {
            brands
                .as_chunks::<4>()
                .0
                .iter()
                .take(MAX_COMPATIBLE_BRANDS)
                .any(|brand| is_image(brand))
        })
}

/// Whether a TIFF's first image is only a small copy of the real one, which is
/// how camera RAW files (NEF, CR2, ARW, DNG, and the rest) lay themselves out.
/// Reading one as a TIFF would quietly return that small copy.
///
/// Any one of these in the first directory says so:
///
/// - `NewSubfileType` with its lowest bit set: the file itself calls the first
///   image a reduced-resolution copy of another.
/// - `DNGVersion`: the file says it is a DNG, whose first image is a preview.
/// - `CR` after the header: Canon's CR2, which marks itself nowhere else.
/// - `SubIFDs` and `Make` together, on a first image no longer than
///   [`RAW_THUMBNAIL_MAX_EDGE_PX`]: a camera wrote it, the full image lives in
///   another directory, and the first one is thumbnail sized.
///
/// `SubIFDs` alone proves nothing: a pyramidal TIFF keeps its smaller levels
/// there, under a first image that is the whole picture, and is read here.
///
/// What is left over: a TIFF of at most that edge which carries both a `Make`
/// and `SubIFDs` is taken for a RAW file and handed to the system, which
/// refuses a plain TIFF; and a RAW file that marks itself in none of these ways
/// is read as whatever its first directory holds.
pub(crate) fn tiff_holds_only_a_preview(input: &[u8]) -> bool {
    let read = |offset: usize, length: usize| input.get(offset..offset.checked_add(length)?);
    let little_endian = match read(0, 2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return false,
    };
    if read(8, 2) == Some(b"CR") {
        return true;
    }
    let u16_at = |offset: usize| {
        read(offset, 2).map(|bytes| {
            let bytes = [bytes[0], bytes[1]];
            if little_endian {
                u16::from_le_bytes(bytes)
            } else {
                u16::from_be_bytes(bytes)
            }
        })
    };
    let u32_at = |offset: usize| {
        read(offset, 4).map(|bytes| {
            let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if little_endian {
                u32::from_le_bytes(bytes)
            } else {
                u32::from_be_bytes(bytes)
            }
        })
    };
    let Some(directory) = u32_at(4).map(|offset| offset as usize) else {
        return false;
    };
    let Some(entries) = u16_at(directory) else {
        return false;
    };
    // A SHORT or a LONG, both of which sit at the start of the value field.
    let number_at = |entry: usize| match u16_at(entry + 2) {
        Some(3) => u16_at(entry + 8).map(u32::from),
        Some(4) => u32_at(entry + 8),
        _ => None,
    };
    let (mut make, mut sub_directories) = (false, false);
    let (mut width, mut height) = (None, None);
    for index in 0..usize::from(entries) {
        let entry = directory + 2 + index * 12;
        match u16_at(entry) {
            Some(0x00fe) if u32_at(entry + 8).is_some_and(|value| value & 1 == 1) => return true,
            Some(0xc612) => return true,
            Some(0x0100) => width = number_at(entry),
            Some(0x0101) => height = number_at(entry),
            Some(0x010f) => make = true,
            Some(0x014a) => sub_directories = true,
            _ => {}
        }
    }
    let thumbnail_sized = matches!(
        (width, height),
        (Some(width), Some(height)) if width.max(height) <= RAW_THUMBNAIL_MAX_EDGE_PX
    );
    make && sub_directories && thumbnail_sized
}
