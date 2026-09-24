use super::*;
use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};
use serde_json::{json, Map, Value};
use std::{
    io::Cursor,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use ureq::unversioned::multipart::{Form, Part};

fn response_json(mut response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    let bytes = response.body_mut().read_to_vec()?;
    serde_json::from_slice(&bytes).context("parse ComfyUI response JSON")
}

fn post_json(agent: &ureq::Agent, url: String, value: Value) -> Result<Value> {
    let response = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(serde_json::to_vec(&value)?)?;
    response_json(response)
}

pub const DEFAULT_COMFY_PROMPT: &str = "In <image1>, remove only the subject shown in white in the mask <image2>. Fill the removed area naturally from the surrounding context. Preserve the rest of the scene, lighting, texture, and perspective.";
const INPUT_EDGE: u32 = 1024;
const SOLID_MARGIN_MODEL_PX: f32 = 8.0;
const OUTER_MARGIN_MODEL_PX: f32 = 18.0;
const FEATHER_SIGMA_MODEL_PX: f32 = 4.0;
const MAX_WORKFLOW_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct ComfyRemoveConfig {
    pub url: String,
    pub prompt: String,
    pub workflow: Option<String>,
    pub context_scale: u32,
}

fn workflow_from_str(source: &str) -> Result<Value> {
    anyhow::ensure!(
        source.len() <= MAX_WORKFLOW_BYTES,
        "ComfyUI workflow is too large"
    );
    let value: Value = serde_json::from_str(source).context("parse ComfyUI workflow JSON")?;
    let mut graph = if value.get("nodes").is_some() {
        convert_ui_workflow(&value)?
    } else {
        value
            .get("prompt")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or(value)
    };
    validate_graph(&mut graph)?;
    Ok(graph)
}

pub fn validate_comfy_workflow(source: &str) -> Result<()> {
    workflow_from_str(source).map(|_| ())
}

fn convert_ui_workflow(value: &Value) -> Result<Value> {
    let nodes = value["nodes"]
        .as_array()
        .context("UI workflow has no nodes")?;
    let links = value["links"]
        .as_array()
        .context("UI workflow has no links")?;
    let mut link_sources = std::collections::HashMap::new();
    for link in links {
        let parts = link.as_array().context("invalid UI workflow link")?;
        if parts.len() >= 4 {
            let id = parts[0].as_u64().context("invalid UI link ID")?;
            let source = parts[1].as_u64().context("invalid UI source ID")?;
            let slot = parts[2].as_u64().context("invalid UI source slot")?;
            link_sources.insert(id, json!([source.to_string(), slot]));
        }
    }
    let mut graph = Map::new();
    for node in nodes {
        let id = node["id"]
            .as_u64()
            .context("UI node has no ID")?
            .to_string();
        let kind = node["type"].as_str().context("UI node has no type")?;
        // UI exports do not name widget inputs for arbitrary custom nodes. The
        // supplied Qwen 2.1 graph uses named widgets, so it can be converted safely.
        let widgets = node.get("widgets_values_named").and_then(Value::as_object);
        if node.get("widgets_values").is_some_and(|v| !v.is_null()) && widgets.is_none() {
            anyhow::bail!(
                "UI node {id} ({kind}) has unnamed widgets; export this workflow in API format"
            );
        }
        let mut inputs = Map::new();
        if let Some(widgets) = widgets {
            for (name, value) in widgets {
                if name != "upload" && name != "control_after_generate" {
                    inputs.insert(name.clone(), value.clone());
                }
            }
        }
        if let Some(sockets) = node["inputs"].as_array() {
            for socket in sockets {
                if let (Some(name), Some(link)) = (socket["name"].as_str(), socket["link"].as_u64())
                {
                    let source = link_sources
                        .get(&link)
                        .context("UI workflow has a missing link")?;
                    inputs.insert(name.to_owned(), source.clone());
                }
            }
        }
        graph.insert(id, json!({"class_type":kind,"inputs":inputs}));
    }
    Ok(Value::Object(graph))
}

fn validate_graph(graph: &mut Value) -> Result<()> {
    let nodes = graph
        .as_object_mut()
        .context("workflow must be a ComfyUI API graph or UI export")?;
    anyhow::ensure!(
        !nodes.is_empty() && nodes.len() <= 256,
        "ComfyUI workflow has an invalid node count"
    );
    let mut images = 0;
    let mut masks = 0;
    let mut prompts = 0;
    let mut outputs = 0;
    for node in nodes.values_mut() {
        let kind = node["class_type"]
            .as_str()
            .context("workflow node has no class_type")?
            .to_owned();
        let inputs = node["inputs"]
            .as_object_mut()
            .context("workflow node has no inputs")?;
        match kind.as_str() {
            "LoadImage" => {
                images += 1;
                inputs.insert("image".into(), json!("CALIBRAW_IMAGE"));
            }
            "LoadImageMask" => {
                masks += 1;
                inputs.insert("image".into(), json!("CALIBRAW_MASK"));
                inputs.insert("channel".into(), json!("red"));
            }
            "TextEncodeQwenImage21" => {
                prompts += 1;
            }
            "SaveImage" => {
                outputs += 1;
                inputs.insert("filename_prefix".into(), json!("CalibRaw_Remove"));
            }
            _ => {}
        }
    }
    anyhow::ensure!(images == 1 && masks == 1 && prompts == 1 && outputs == 1,
        "workflow needs one LoadImage, one LoadImageMask, one TextEncodeQwenImage21, and one SaveImage node");
    Ok(())
}

fn set_inputs(graph: &mut Value, image: &str, mask: &str, prompt: &str) {
    for node in graph.as_object_mut().unwrap().values_mut() {
        match node["class_type"].as_str().unwrap_or("") {
            "LoadImage" => node["inputs"]["image"] = json!(image),
            "LoadImageMask" => node["inputs"]["image"] = json!(mask),
            "TextEncodeQwenImage21" => node["inputs"]["prompt"] = json!(prompt),
            "KSampler" => {
                if node["inputs"].get("seed").is_some() {
                    node["inputs"]["seed"] = json!(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos() as u64
                            % 1_000_000_000_000_000
                    );
                }
            }
            _ => {}
        }
    }
}

fn png_bytes(image: DynamicImage) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .context("encode ComfyUI input PNG")?;
    Ok(bytes.into_inner())
}

#[derive(Clone, Copy, Debug)]
struct ChannelMatch {
    gain: f32,
    offset: f32,
}

fn median(values: &mut [f32]) -> f32 {
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

/// Qwen often regrades the complete reference image, including the unchanged
/// context. Measure that regrade just outside the edit and undo it before the
/// generated pixels are converted back to the RAW scene. The ring follows the
/// mask instead of averaging unrelated parts of the 1024 px crop.
fn match_comfy_context_colors(
    source: &RgbImage,
    generated: &Rgb32FImage,
    mask: &GrayImage,
) -> Rgb32FImage {
    let dimensions = source.dimensions();
    if generated.dimensions() != dimensions || mask.dimensions() != dimensions {
        return generated.clone();
    }
    let ring = image::imageops::blur(mask, 12.0);
    let mut samples = Vec::new();
    for y in (0..dimensions.1).step_by(2) {
        for x in (0..dimensions.0).step_by(2) {
            if mask.get_pixel(x, y)[0] != 0 || ring.get_pixel(x, y)[0] < 3 {
                continue;
            }
            let before = source.get_pixel(x, y);
            let after = generated.get_pixel(x, y);
            // Qwen may alter nearby unmasked objects. Their pixels are not a
            // color reference for the replacement background.
            if after.0.iter().all(|value| value.is_finite())
                && (0..3)
                    .all(|channel| (before[channel] as f32 / 255.0 - after[channel]).abs() < 0.20)
            {
                samples.push((before.0.map(|value| value as f32 / 255.0), after.0));
            }
        }
    }
    let channels: [ChannelMatch; 3] = std::array::from_fn(|channel| {
        if samples.len() < 64 {
            return ChannelMatch {
                gain: 1.0,
                offset: 0.0,
            };
        }
        let mut differences = samples
            .iter()
            .map(|(before, after)| before[channel] - after[channel])
            .collect::<Vec<_>>();
        let shift = median(&mut differences);
        let mut sum = [0.0f64; 5];
        for (before, after) in &samples {
            if (before[channel] - after[channel] - shift).abs() > 0.08 {
                continue;
            }
            let x = after[channel] as f64;
            let y = before[channel] as f64;
            sum[0] += 1.0;
            sum[1] += x;
            sum[2] += y;
            sum[3] += x * x;
            sum[4] += x * y;
        }
        let mut gain = 1.0;
        if sum[0] >= 64.0 {
            let variance = sum[3] - sum[1] * sum[1] / sum[0];
            let covariance = sum[4] - sum[1] * sum[2] / sum[0];
            // Fit contrast only when the ring has enough variation and the
            // generated context still follows the original spatially.
            if variance > sum[0] * 0.0025 && covariance > variance * 0.5 {
                gain = (covariance / variance).clamp(0.75, 1.33) as f32;
            }
        }
        let mut offsets = samples
            .iter()
            .map(|(before, after)| before[channel] - gain * after[channel])
            .collect::<Vec<_>>();
        ChannelMatch {
            gain,
            offset: median(&mut offsets).clamp(-0.30, 0.30),
        }
    });
    let matched: Rgb32FImage = ImageBuffer::from_fn(dimensions.0, dimensions.1, |x, y| {
        let pixel = generated.get_pixel(x, y);
        Rgb(std::array::from_fn(|channel| {
            (pixel[channel] * channels[channel].gain + channels[channel].offset).clamp(0.0, 1.0)
        }))
    });

    // Some Qwen runs preserve the unmasked reference exactly but create a
    // differently graded fill. Compare pixels on either side of the painted
    // edge as a second, local estimate. A consensus check avoids correcting
    // when the edge crosses several unrelated materials.
    let mut boundary = [Vec::new(), Vec::new(), Vec::new()];
    for y in 1..dimensions.1.saturating_sub(1) {
        for x in 1..dimensions.0.saturating_sub(1) {
            if mask.get_pixel(x, y)[0] == 0 {
                continue;
            }
            let inside = matched.get_pixel(x, y);
            for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if mask.get_pixel(nx, ny)[0] != 0 {
                    continue;
                }
                let outside = source.get_pixel(nx, ny);
                let generated_outside = matched.get_pixel(nx, ny);
                if (0..3).any(|channel| {
                    (outside[channel] as f32 / 255.0 - generated_outside[channel]).abs() > 0.10
                }) {
                    continue;
                }
                for channel in 0..3 {
                    boundary[channel].push(outside[channel] as f32 / 255.0 - inside[channel]);
                }
            }
        }
    }
    let offsets: [f32; 3] = std::array::from_fn(|channel| {
        let values = &mut boundary[channel];
        if values.len() < 64 {
            return 0.0;
        }
        let middle = median(values);
        let agreeing = values
            .iter()
            .filter(|value| (**value - middle).abs() < 0.06)
            .count();
        if agreeing * 3 < values.len() {
            0.0
        } else {
            middle.clamp(-0.25, 0.25)
        }
    });
    if offsets.iter().all(|offset| offset.abs() < 1.0 / 255.0) {
        return matched;
    }
    ImageBuffer::from_fn(dimensions.0, dimensions.1, |x, y| {
        let pixel = matched.get_pixel(x, y);
        if mask.get_pixel(x, y)[0] == 0 {
            *pixel
        } else {
            Rgb(std::array::from_fn(|channel| {
                (pixel[channel] + offsets[channel]).clamp(0.0, 1.0)
            }))
        }
    })
}

fn upload(agent: &ureq::Agent, base: &str, name: &str, png: Vec<u8>) -> Result<String> {
    let form = Form::new()
        .part(
            "image",
            Part::bytes(&png).file_name(name).mime_str("image/png")?,
        )
        .text("type", "input")
        .text("overwrite", "false");
    let value: Value = response_json(
        agent
            .post(format!("{base}/upload/image"))
            .send(form)
            .context("upload image to ComfyUI")?,
    )?;
    let filename = value["name"]
        .as_str()
        .context("ComfyUI upload returned no filename")?;
    Ok(filename.to_owned())
}

fn request_comfy_free(agent: &ureq::Agent, base: &str) -> Result<()> {
    let body = br#"{"unload_models":true,"free_memory":true}"#;
    agent
        .post(format!("{base}/free"))
        .header("Content-Type", "application/json")
        .send(body.as_slice())
        .context("request ComfyUI model unload")?;
    Ok(())
}

fn free_comfy(agent: &ureq::Agent, base: &str) {
    if let Err(error) = request_comfy_free(agent, base) {
        log::warn!("ComfyUI could not unload models after Remove: {error}");
    }
}

pub(super) fn run_comfy_remove(
    mut request: RemoveRequest,
    events: &mpsc::Sender<RemoveEvent>,
) -> Result<RemoveStroke> {
    let config = request.comfy.as_ref().context("missing ComfyUI settings")?;
    let base = config.url.trim().trim_end_matches('/');
    anyhow::ensure!(
        base.starts_with("http://") || base.starts_with("https://"),
        "ComfyUI URL must start with http:// or https://"
    );
    anyhow::ensure!(
        !base.contains('?') && !base.contains('#'),
        "ComfyUI URL must be a server address"
    );
    let mut graph = workflow_from_str(
        config
            .workflow
            .as_deref()
            .unwrap_or(include_str!("../qwen_remove_default.json")),
    )?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .build()
        .into();
    // A previous ComfyUI job may still have Qwen resident. Release it before
    // CalibRaw allocates the temporary GPU graph used to render this crop.
    request_comfy_free(&agent, base).context("connect to ComfyUI before rendering")?;
    std::thread::sleep(Duration::from_millis(1200));
    ensure_not_cancelled(&request.cancellation)?;
    let prompt = if config.prompt.trim().is_empty() {
        DEFAULT_COMFY_PROMPT
    } else {
        config.prompt.trim()
    };
    let mut brush = request.brush.clone();
    if brush.dilation_radius == 0 {
        brush.dilation_radius = adaptive_remove_dilation(&brush.points);
    }
    let mask = rasterize_remove_brush(request.raw.width, request.raw.height, &brush)
        .context("ComfyUI brush produced no native image mask")?;
    let crop = plan_remove_context_crop_with_scale(
        request.raw.width,
        request.raw.height,
        &mask,
        config.context_scale.clamp(3, 8),
        INPUT_EDGE,
    )
    .context("ComfyUI mask produced no context crop")?;
    let scene = render_remove_scene_crop_resized(
        DevelopedCropJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: request.geometry,
            exposure: request.exposure,
            masks: request.masks.clone(),
            remove: request.existing.clone(),
            crop,
            program_prewarm: request.program_prewarm.clone(),
        },
        INPUT_EDGE,
    )
    .context("render ComfyUI context crop")?;
    request.program_prewarm = None;
    ensure_not_cancelled(&request.cancellation)?;
    let view_gain = remove_model_view_gain(&request.raw, &scene.pixels);
    let rgb: RgbImage = ImageBuffer::from_fn(scene.width, scene.height, |x, y| {
        let i = (y as usize * scene.width as usize + x as usize) * 3;
        let s = remove_scene_to_model_srgb(
            &request.raw,
            [scene.pixels[i], scene.pixels[i + 1], scene.pixels[i + 2]],
            view_gain,
        );
        Rgb(s.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
    });
    let image = image::imageops::resize(&rgb, INPUT_EDGE, INPUT_EDGE, FilterType::Lanczos3);
    // The painted area must be fully replaced. Extend it enough to catch
    // wispy edges, then reserve a separate outer ring for the fade. The crop
    // is planned from the original mask so both margins stay in the context.
    let model_scale = crop.width.max(crop.height) as f32 / INPUT_EDGE as f32;
    let padded_mask = |model_px: f32| -> Result<GrayImage> {
        let mut padded_brush = brush.clone();
        let extra = (model_px * model_scale).round().max(1.0) as u32;
        padded_brush.dilation_radius = padded_brush.dilation_radius.saturating_add(extra);
        let padded = rasterize_remove_brush(request.raw.width, request.raw.height, &padded_brush)
            .context("ComfyUI padded brush produced no mask")?;
        Ok(crop_binary_mask(crop, &padded))
    };
    let solid_mask = padded_mask(SOLID_MARGIN_MODEL_PX)?;
    let source_mask = padded_mask(OUTER_MARGIN_MODEL_PX)?;
    let mask_image =
        image::imageops::resize(&source_mask, INPUT_EDGE, INPUT_EDGE, FilterType::Nearest);
    anyhow::ensure!(
        mask_image.pixels().any(|p| p[0] > 0),
        "ComfyUI mask vanished during resize"
    );
    let image_png = png_bytes(DynamicImage::ImageRgb8(image.clone()))?;
    let mask_png = png_bytes(DynamicImage::ImageLuma8(mask_image.clone()))?;
    let source_scene: Rgb32FImage = ImageBuffer::from_raw(scene.width, scene.height, scene.pixels)
        .context("construct ComfyUI source scene")?;

    // The UI drops its GPU preview after the crop has been read back. Avoid
    // running a large Qwen model concurrently with CalibRaw's preview graph.
    events
        .send(RemoveEvent::ComfyReady)
        .context("ComfyUI handoff receiver closed")?;
    while !request.comfy_handoff.load(Ordering::Acquire) {
        ensure_not_cancelled(&request.cancellation)?;
        std::thread::sleep(Duration::from_millis(25));
    }
    let mut submitted_id: Option<String> = None;
    let result = (|| -> Result<RemoveStroke> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let image_name = upload(
            &agent,
            base,
            &format!("calibraw_{stamp}_image.png"),
            image_png,
        )?;
        ensure_not_cancelled(&request.cancellation)?;
        let mask_name = upload(
            &agent,
            base,
            &format!("calibraw_{stamp}_mask.png"),
            mask_png,
        )?;
        set_inputs(&mut graph, &image_name, &mask_name, prompt);
        let response: Value = post_json(
            &agent,
            format!("{base}/prompt"),
            json!({"prompt":graph,"client_id":"calibraw-remove"}),
        )
        .context("queue ComfyUI workflow")?;
        let id = response["prompt_id"]
            .as_str()
            .context("ComfyUI returned no prompt ID")?;
        submitted_id = Some(id.to_owned());
        let started = Instant::now();
        let output_info = loop {
            ensure_not_cancelled(&request.cancellation)?;
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(15 * 60),
                "ComfyUI workflow timed out"
            );
            let history: Value = response_json(
                agent
                    .get(format!("{base}/history/{id}"))
                    .call()
                    .context("poll ComfyUI history")?,
            )?;
            if let Some(entry) = history.get(id) {
                if entry["status"]["status_str"] == "error" {
                    anyhow::bail!("ComfyUI workflow failed: {}", entry["status"]);
                }
                if let Some(outputs) = entry["outputs"].as_object() {
                    let save_id = graph
                        .as_object()
                        .unwrap()
                        .iter()
                        .find(|(_, n)| n["class_type"] == "SaveImage")
                        .map(|(id, _)| id)
                        .unwrap();
                    if let Some(first) = outputs
                        .get(save_id)
                        .and_then(|v| v["images"].as_array())
                        .and_then(|v| v.first())
                    {
                        break first.clone();
                    }
                }
                if entry["status"]["completed"] == true {
                    anyhow::bail!("ComfyUI completed without a SaveImage output");
                }
            }
            std::thread::sleep(Duration::from_millis(750));
        };
        let filename = output_info["filename"]
            .as_str()
            .context("ComfyUI output has no filename")?;
        let subfolder = output_info["subfolder"].as_str().unwrap_or("");
        let kind = output_info["type"].as_str().unwrap_or("output");
        let bytes = agent
            .get(format!("{base}/view"))
            .query("filename", filename)
            .query("subfolder", subfolder)
            .query("type", kind)
            .call()
            .context("download ComfyUI result")?
            .body_mut()
            .with_config()
            .limit(MAX_OUTPUT_BYTES as u64)
            .read_to_vec()?;
        anyhow::ensure!(
            bytes.len() <= MAX_OUTPUT_BYTES,
            "ComfyUI output exceeds 32 MB"
        );
        let output = image::load_from_memory(&bytes)
            .context("decode ComfyUI result")?
            .to_rgb32f();
        anyhow::ensure!(
            output.width() == INPUT_EDGE && output.height() == INPUT_EDGE,
            "ComfyUI result must be 1024x1024 pixels, got {}x{}",
            output.width(),
            output.height()
        );
        ensure_not_cancelled(&request.cancellation)?;
        let output = match_comfy_context_colors(&image, &output, &mask_image);
        let patch = build_cached_patch(
            crop,
            &request.raw,
            &request.exposure,
            &source_scene,
            view_gain,
            &output,
            &source_mask,
            INPUT_EDGE,
            PatchFeather::OutsideSolid {
                solid_mask: &solid_mask,
                sigma_model_px: FEATHER_SIGMA_MODEL_PX,
            },
        )?;
        let _ = events.send(RemoveEvent::Processing {
            completed: 1,
            total: 1,
        });
        Ok(RemoveStroke {
            brush,
            patches: vec![patch],
            retouch: None,
            backend: RemoveBackend::Comfy,
            opacity: request.opacity,
        })
    })();
    if result.is_err() {
        if let Some(id) = submitted_id.as_deref() {
            // Newer ComfyUI versions cancel this specific job even when it is
            // already running, without interrupting somebody else's prompt.
            if let Err(error) = agent
                .post(format!("{base}/api/jobs/{id}/cancel"))
                .send("".as_bytes())
            {
                log::warn!("Could not cancel ComfyUI Remove job {id}: {error}");
            }
        }
    }
    free_comfy(&agent, base);
    // /free schedules the unload on ComfyUI's worker loop. Give it a chance
    // to release VRAM before the UI recreates its preview pipeline.
    std::thread::sleep(Duration::from_millis(1200));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_color_match_removes_qwen_cast_without_flattening_generated_detail() {
        let source = RgbImage::from_fn(96, 96, |x, y| Rgb([90 + x as u8, 125 + y as u8 / 2, 190]));
        let generated = Rgb32FImage::from_fn(96, 96, |x, y| {
            let original = source.get_pixel(x, y).0.map(|v| v as f32 / 255.0);
            Rgb([
                original[0] * 0.8 + 0.02,
                original[1] - 0.10,
                original[2] - 0.15,
            ])
        });
        let mask = GrayImage::from_fn(96, 96, |x, y| {
            image::Luma([u8::from((24..72).contains(&x) && (24..72).contains(&y)) * 255])
        });
        let matched = match_comfy_context_colors(&source, &generated, &mask);
        for channel in 0..3 {
            let expected = source.get_pixel(48, 48)[channel] as f32 / 255.0;
            assert!((matched.get_pixel(48, 48)[channel] - expected).abs() < 0.015);
        }
        let old_detail = generated.get_pixel(60, 48)[0] - generated.get_pixel(36, 48)[0];
        let new_detail = matched.get_pixel(60, 48)[0] - matched.get_pixel(36, 48)[0];
        assert!(new_detail > old_detail);
    }

    #[test]
    fn context_color_match_keeps_output_when_no_unmasked_reference_exists() {
        let source = RgbImage::from_pixel(32, 32, Rgb([100, 150, 200]));
        let generated = Rgb32FImage::from_pixel(32, 32, Rgb([0.2, 0.3, 0.4]));
        let mask = GrayImage::from_pixel(32, 32, image::Luma([255]));
        assert_eq!(
            match_comfy_context_colors(&source, &generated, &mask),
            generated
        );
    }

    #[test]
    fn context_color_match_fixes_fill_only_cast_when_outer_context_is_unchanged() {
        let source = RgbImage::from_pixel(96, 96, Rgb([120, 165, 205]));
        let mask = GrayImage::from_fn(96, 96, |x, y| {
            image::Luma([u8::from((24..72).contains(&x) && (24..72).contains(&y)) * 255])
        });
        let generated = Rgb32FImage::from_fn(96, 96, |x, y| {
            let source_color = source.get_pixel(x, y).0.map(|value| value as f32 / 255.0);
            if mask.get_pixel(x, y)[0] > 0 {
                Rgb(source_color.map(|value| value - 0.14))
            } else {
                Rgb(source_color)
            }
        });
        let matched = match_comfy_context_colors(&source, &generated, &mask);
        for channel in 0..3 {
            let expected = source.get_pixel(48, 48)[channel] as f32 / 255.0;
            assert!((matched.get_pixel(48, 48)[channel] - expected).abs() < 0.01);
        }
        assert_eq!(matched.get_pixel(12, 12), generated.get_pixel(12, 12));
    }

    #[test]
    fn context_color_match_ignores_restructured_unmasked_subject() {
        let source = RgbImage::from_fn(128, 128, |x, _| {
            if x < 56 {
                Rgb([35, 30, 28])
            } else {
                Rgb([178, 174, 170])
            }
        });
        let mask = GrayImage::from_fn(128, 128, |x, y| {
            image::Luma([u8::from((56..76).contains(&x) && (16..112).contains(&y)) * 255])
        });
        let generated = Rgb32FImage::from_fn(128, 128, |x, y| {
            if x < 56 {
                Rgb([0.70, 0.68, 0.67])
            } else if mask.get_pixel(x, y)[0] != 0 {
                Rgb([0.62, 0.60, 0.58])
            } else {
                Rgb([0.72, 0.70, 0.68])
            }
        });
        let matched = match_comfy_context_colors(&source, &generated, &mask);
        let pixel = matched.get_pixel(65, 64);
        for channel in 0..3 {
            let expected = source.get_pixel(90, 64)[channel] as f32 / 255.0;
            assert!((pixel[channel] - expected).abs() < 0.025);
        }
    }
    #[test]
    fn default_workflow_has_required_inputs() {
        validate_comfy_workflow(include_str!("../qwen_remove_default.json")).unwrap();
    }

    #[test]
    fn ui_workflow_conversion_replaces_image_and_mask_without_touching_links() {
        let source = json!({
            "nodes": [
                {"id":1,"type":"LoadImage","inputs":[],"widgets_values_named":{"image":"old.png","upload":"image"}},
                {"id":2,"type":"LoadImageMask","inputs":[],"widgets_values_named":{"image":"old-mask.png","channel":"alpha"}},
                {"id":3,"type":"TextEncodeQwenImage21","inputs":[{"name":"images.image_1","link":10}],"widgets_values_named":{"prompt":"old","resolution":0}},
                {"id":4,"type":"SaveImage","inputs":[],"widgets_values_named":{"filename_prefix":"old"}}
            ],
            "links":[[10,1,0,3,0,"IMAGE"]]
        });
        let graph = workflow_from_str(&source.to_string()).unwrap();
        assert_eq!(graph["1"]["inputs"]["image"], "CALIBRAW_IMAGE");
        assert_eq!(graph["2"]["inputs"]["image"], "CALIBRAW_MASK");
        assert_eq!(graph["2"]["inputs"]["channel"], "red");
        assert_eq!(graph["3"]["inputs"]["images.image_1"], json!(["1", 0]));
    }
}
