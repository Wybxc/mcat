use crate::{
    Frame,
    term_misc::{self, ensure_space},
};
use std::{
    io::{BufRead, Write},
    time::Duration,
};

/// Renders an image as colored ASCII in the terminal using upper/lower half-blocks.
///
/// This function converts an RGBA image into a terminal-friendly representation
/// using ANSI escape codes and colored Unicode half-block characters. It filters
/// out low-luminance, low-opacity pixels to suppress thin outlines and noise,
/// keeping only visually meaningful parts of the image.
///
/// # Arguments
/// - `img`: Image byte slice (any format supported by `image` crate, e.g., PNG, JPEG)
/// - `out`: A writer to send output to (e.g., `std::io::stdout`)
/// - `offset`: Optional horizontal offset in terminal columns (used for centering)
/// - `print_at`: Optional locaiton the image should be printed at
///
/// # Example
/// ```
/// use std::path::Path;
/// use std::io::{self, Write};
/// use rasteroid::ascii_encoder::encode_image;
///
/// let path = Path::new("image.png");
/// let bytes = match std::fs::read(path) {
///     Ok(bytes) => bytes,
///     Err(_) => return,
/// };
///
/// let mut stdout = io::stdout();
/// encode_image(&bytes, &mut stdout, Some(80), None).unwrap();
/// stdout.flush().unwrap();
/// ```
pub fn encode_image(
    img: &[u8],
    out: &mut impl Write,
    offset: Option<u16>,
    print_at: Option<(u16, u16)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let image = image::load_from_memory(img)?;
    let rgba_image = image.to_rgba8();

    let w = rgba_image.width() as usize;
    let h = rgba_image.height() as usize;
    let h_adjusted = if h % 2 == 1 { h - 1 } else { h };

    // Luminance threshold: tweak this to suppress small sparkles
    const LUM_THRESHOLD: f32 = 35.0;

    let mut last_max_height = 0;
    for y in (0..h_adjusted).step_by(2) {
        if let Some(at) = print_at {
            let at = (at.0, at.1 + (y / 2) as u16);
            last_max_height = y;
            let loc = term_misc::loc_to_terminal(Some(at));
            out.write_all(loc.as_ref())?;
        }
        if let Some(off) = offset {
            let center = term_misc::offset_to_terminal(Some(off));
            out.write_all(center.as_ref())?;
        }

        for x in 0..w {
            let upper = rgba_image.get_pixel(x as u32, y as u32);
            let lower = rgba_image.get_pixel(x as u32, (y + 1) as u32);

            let (ru, gu, bu, au) = (upper[0], upper[1], upper[2], upper[3]);
            let (rl, gl, bl, al) = (lower[0], lower[1], lower[2], lower[3]);

            const MIN_VISUAL_WEIGHT: f32 = 25.0; // tweak for strictness

            let upper_w = visual_weight(ru, gu, bu, au);
            let lower_w = visual_weight(rl, gl, bl, al);

            let upper_visible = upper_w > MIN_VISUAL_WEIGHT;
            let lower_visible = lower_w > MIN_VISUAL_WEIGHT;

            match (upper_visible, lower_visible) {
                (false, false) => {
                    out.write_all(b" ")?;
                }
                (true, false) => {
                    write!(out, "\x1b[38;2;{};{};{}m▀\x1b[0m", ru, gu, bu)?;
                }
                (false, true) => {
                    write!(out, "\x1b[38;2;{};{};{}m▄\x1b[0m", rl, gl, bl)?;
                }
                (true, true) => {
                    // Use dual-color upper/lower block
                    write!(
                        out,
                        "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀\x1b[0m",
                        ru, gu, bu, rl, gl, bl
                    )?;
                }
            }
        }

        out.write_all(b"\n")?;
    }

    if h % 2 == 1 {
        if let Some(at) = print_at {
            let add_y = (last_max_height + 2) / 2;
            let at = (at.0, at.1 + add_y as u16);
            let loc = term_misc::loc_to_terminal(Some(at));
            out.write_all(loc.as_ref())?;
        }
        if let Some(off) = offset {
            let center = term_misc::offset_to_terminal(Some(off));
            out.write_all(center.as_ref())?;
        }

        for x in 0..w {
            let p = rgba_image.get_pixel(x as u32, (h - 1) as u32);
            let (r, g, b, a) = (p[0], p[1], p[2], p[3]);
            let lum = luminance(r, g, b);

            if a == 0 || lum < LUM_THRESHOLD {
                out.write_all(b" ")?;
            } else {
                write!(out, "\x1b[38;2;{};{};{}m▀\x1b[0m", r, g, b)?;
            }
        }

        out.write_all(b"\n")?;
    }

    out.write_all(b"\x1b[0m")?;
    Ok(())
}

fn luminance(r: u8, g: u8, b: u8) -> f32 {
    0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
}

fn visual_weight(r: u8, g: u8, b: u8, a: u8) -> f32 {
    if a == 0 {
        0.0
    } else {
        luminance(r, g, b) * (a as f32 / 255.0)
    }
}

/// Streams a sequence of video frames to the terminal as colored ASCII art.
///
/// This function takes an iterator over video frames and renders them
/// to the terminal using ANSI escape codes and ASCII half-block characters.
/// It respects frame timestamps for playback timing, optionally centers
/// each frame horizontally, and avoids flickering by redrawing only the
/// image area.
///
/// # Arguments
/// - `frames`: A mutable iterator over items implementing the `Frame` trait.
/// - `out`: A writer to send output to (e.g., `std::io::stdout()`).
/// - `center`: If `true`, horizontally centers each frame in the terminal.
/// - `cycle`: If `true`, will loop over the animation until interrupted.
///
/// # Notes
/// Each frame is expected to contain encoded image bytes (e.g., PNG, JPEG).
/// This should include any kind of img that the image crate supports.
/// This function decodes the image using the `image` crate before rendering.
///
/// # Example
/// first make sure you can supply a iter of Frames (using ffmpeg-sidecar here)
/// ```rust,no_run
/// use ffmpeg_sidecar::command::FfmpegCommand;
/// use rasteroid::Frame;
/// use ffmpeg_sidecar::event::OutputVideoFrame;
/// use rasteroid::kitty_encoder::encode_frames;
/// use rasteroid::image_extended::calc_fit;
/// use ffmpeg_sidecar::event::FfmpegEvent;
/// use rasteroid::image_extended::InlineImage;
///
/// pub struct AsciiFrames {
///     frame: OutputVideoFrame,
///     img: Vec<u8>,
/// }
/// impl Frame for AsciiFrames {
///     fn timestamp(&self) -> f32 {
///         self.frame.timestamp
///     }
///     // needs to be something image crate can load.
///     fn data(&self) -> &[u8] {
///         &self.img
///     }
///     // doesn't matter here
///     fn width(&self) -> u16 {
///         0
///     }
///     // doesn't matter here
///     fn height(&self) -> u16 {
///         0
///     }
/// }
/// // next get the frames (taken from ffmpeg-sidecar)
///
/// let mut out = std::io::stdout();
/// let iter = match FfmpegCommand::new() // <- Builder API like `std::process::Command`
///   .testsrc()  // <- Discoverable aliases for FFmpeg args
///   .rawvideo() // <- Convenient argument presets
///   .spawn()    // <- Ordinary `std::process::Child`
///    {
///        Ok(res) => res,
///        Err(e) => return,
/// }.iter().unwrap();   // <- Blocking iterator over logs and output
/// // now convert to compatible frames
/// let width = Some("80%");
/// let height = Some("80%");
/// let center = true;
/// let mut ascii_frames = iter.filter_map(|event| {
///     if let FfmpegEvent::OutputFrame(f) = event {
///        let rgb_image = image::RgbImage::from_raw(f.width, f.height, f.data.clone())
///            .unwrap();
///        let img = image::DynamicImage::ImageRgb8(rgb_image);
///        let (img, _, _, _) = img.resize_plus(width, height, true, false).unwrap();
///        Some(AsciiFrames { frame: f, img })
///     } else {
///        None
///     }
/// });
/// rasteroid::ascii_encoder::encode_frames(&mut ascii_frames, out, center, false).unwrap();
/// ```
pub fn encode_frames(
    frames: &mut dyn Iterator<Item = impl Frame>,
    mut out: impl Write,
    center: bool,
    cycle: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut last_timestamp = None;
    let mut frame_outputs = Vec::new();
    let mut start = true;

    for frame in frames {
        let data = frame.data();
        if data.is_empty() {
            continue;
        }
        let img = image::load_from_memory(data)?;
        let offset = term_misc::center_image(img.width() as u16, true);

        let target_delay = match (frame.timestamp(), last_timestamp) {
            (ts, Some(last)) if ts > last => Duration::from_secs_f32(ts - last),
            _ => Duration::from_millis(33), // default ~30fps
        };
        last_timestamp = Some(frame.timestamp());

        let mut buffer = Vec::new();

        encode_image(
            data,
            &mut buffer,
            if center { Some(offset) } else { None },
            None,
        )?;

        clear_write_frame(&mut out, &buffer, start)?;
        start = false;

        out.flush()?;

        frame_outputs.push((buffer, target_delay));
        std::thread::sleep(target_delay);
    }

    if frame_outputs.is_empty() {
        return Ok(());
    }

    if cycle {
        loop {
            for (output, delay) in &frame_outputs {
                clear_write_frame(&mut out, output, false)?;
                out.flush()?;
                std::thread::sleep(*delay);
            }
        }
    } else {
        Ok(())
    }
}

fn clear_write_frame(
    mut out: impl Write,
    val: &Vec<u8>,
    start: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    if start {
        let image_height = val.lines().count();
        ensure_space(&mut buf, image_height as u16)?;
        buf.extend_from_slice(b"\x1b[s");
    } else {
        buf.extend_from_slice(b"\x1b[u");
        buf.extend_from_slice(b"\x1b[s");
    }
    buf.extend_from_slice(val);

    out.write_all(&buf)?;

    Ok(())
}
