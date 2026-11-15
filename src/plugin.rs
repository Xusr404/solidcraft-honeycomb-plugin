use super::{
    geometry::{self, polygon_area, Orientation, Point},
    parameter_names::{ParameterList, ParameterNames},
};
use craft::plugin::{
    commands, control, log,
    types::{FieldFailure, PluginError},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use wit_bindgen::generate;

generate!({ path: "wit", world: "command-plugin" });

const HOST_CALL_LIMIT: usize = 256;
// Selection, sketch inspection, parameter listing, and two parameter inserts.
const SETUP_CALLS: usize = 5;
// Covers the largest supported cell pitch (500 mm size plus 2000 mm gap).
const MAX_GRID_OFFSET_MM: f64 = 2_500.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    #[serde(default)]
    orientation: Orientation,
    #[serde(default = "default_size")]
    across_flats_mm: f64,
    #[serde(default = "default_gap")]
    gap_mm: f64,
    #[serde(default)]
    rotation_deg: f64,
    #[serde(
        default = "default_include_partial_cells",
        alias = "trim_cells_at_boundary"
    )]
    include_partial_cells: bool,
    #[serde(default)]
    anchor_mode: AnchorMode,
    #[serde(default)]
    offset_x_mm: f64,
    #[serde(default)]
    offset_y_mm: f64,
    #[serde(default)]
    outline_inset_mm: Option<f64>,
    // Keep accepting the separate checkbox and width from older scripts.
    #[serde(default)]
    boundary_inset: bool,
    #[serde(default = "default_boundary_inset")]
    boundary_inset_mm: f64,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
enum AnchorMode {
    #[default]
    #[serde(rename = "Center", alias = "center")]
    Center,
    #[serde(rename = "Selected sketch point", alias = "selected_point")]
    SelectedPoint,
    #[serde(rename = "Bottom left", alias = "bbox_bottom_left")]
    BottomLeft,
    #[serde(rename = "Bottom right", alias = "bbox_bottom_right")]
    BottomRight,
    #[serde(rename = "Top left", alias = "bbox_top_left")]
    TopLeft,
    #[serde(rename = "Top right", alias = "bbox_top_right")]
    TopRight,
}

fn default_size() -> f64 {
    10.0
}
fn default_gap() -> f64 {
    0.5
}
fn default_include_partial_cells() -> bool {
    true
}
fn default_boundary_inset() -> f64 {
    2.0
}

impl Parameters {
    fn validate(&self) -> Result<(), (String, String)> {
        let fail = |field: &str, reason: &str| (field.to_owned(), reason.to_owned());
        if !self.across_flats_mm.is_finite() || !(0.2..=500.0).contains(&self.across_flats_mm) {
            return Err(fail(
                "across_flats_mm",
                "Hexagon size must be between 0.2 and 500 mm.",
            ));
        }
        if !self.gap_mm.is_finite()
            || self.gap_mm < 0.05
            || self.gap_mm > self.across_flats_mm * 4.0
        {
            return Err(fail(
                "gap_mm",
                "Gap must be at least 0.05 mm and no more than four times the hexagon size.",
            ));
        }
        if !(-180.0..=180.0).contains(&self.rotation_deg) {
            return Err(fail(
                "rotation_deg",
                "Rotation must be between -180 and 180 degrees.",
            ));
        }
        if !(0.0..=MAX_GRID_OFFSET_MM).contains(&self.offset_x_mm) {
            return Err(fail(
                "offset_x_mm",
                "X offset must be between 0 and 2500 mm.",
            ));
        }
        if !(0.0..=MAX_GRID_OFFSET_MM).contains(&self.offset_y_mm) {
            return Err(fail(
                "offset_y_mm",
                "Y offset must be between 0 and 2500 mm.",
            ));
        }
        if let Some(width) = self.outline_inset_mm {
            if width != 0.0 && !(0.01..=1_000_000.0).contains(&width) {
                return Err(fail(
                    "outline_inset_mm",
                    "Inset width must be 0 mm or between 0.01 and 1000000 mm.",
                ));
            }
        } else if !(0.01..=1_000_000.0).contains(&self.boundary_inset_mm) {
            return Err(fail(
                "boundary_inset_mm",
                "Border width must be between 0.01 and 1000000 mm.",
            ));
        }
        Ok(())
    }

    fn inset_width(&self) -> f64 {
        self.outline_inset_mm.unwrap_or(if self.boundary_inset {
            self.boundary_inset_mm
        } else {
            0.0
        })
    }
}

struct Boundary {
    sketch_id: Option<u64>,
    sketch_name: String,
    polygons: Vec<Vec<Point>>,
    selected_anchor: Option<Point>,
}

struct Segment {
    id: String,
    a: Point,
    b: Point,
}

struct BoundaryLoop {
    polygon: Vec<Point>,
    curve_ids: HashSet<String>,
}

struct HoneycombPlugin;

impl exports::craft::plugin::entry::Guest for HoneycombPlugin {
    fn execute(entry: String, params: String) -> Result<String, PluginError> {
        if entry != "generate" {
            return Err(PluginError::InvalidInput(FieldFailure {
                field: "entry".into(),
                reason: format!("Unknown entry '{entry}'. Expected 'generate'."),
            }));
        }
        let args: Parameters = serde_json::from_str(&params).map_err(|e| {
            PluginError::InvalidInput(FieldFailure {
                field: "params".into(),
                reason: format!("Invalid generator parameters: {e}"),
            })
        })?;
        args.validate()
            .map_err(|(field, reason)| PluginError::InvalidInput(FieldFailure { field, reason }))?;

        let boundary = read_boundary(args.anchor_mode)?;
        let inset = args.inset_width();
        let use_inset = inset > 0.0;
        let outlines = if use_inset {
            let outlines =
                geometry::inset_outlines(&boundary.polygons, inset).map_err(selection_error)?;
            if outlines.is_empty() {
                return Err(selection_error(
                    "No inset outline fits. Reduce the inset width.",
                ));
            }
            outlines
        } else {
            Vec::new()
        };
        let fill_area = if use_inset {
            &outlines
        } else {
            &boundary.polygons
        };
        let anchor = anchor_point(fill_area, boundary.selected_anchor, &args)?;
        let pattern = geometry::pattern_in_boundary(
            fill_area,
            anchor,
            args.orientation,
            args.across_flats_mm,
            args.gap_mm,
            args.rotation_deg,
            args.include_partial_cells,
            use_inset,
        ).map_err(|limit| match limit {
            geometry::PatternLimit::CellCount => selection_error(&format!(
                "More than {} hexagon cells fit in the selected area. Increase the hexagon size or generate smaller regions separately.",
                geometry::MAX_CELLS
            )),
            geometry::PatternLimit::SearchArea => selection_error(
                "The selected area spans too many grid positions to search in one run. Increase the hexagon size or generate smaller regions separately."
            ),
        })?;
        if pattern.complete_cells.is_empty() && pattern.clipped_paths.is_empty() {
            return Err(PluginError::InvalidInput(FieldFailure {
                field: "selection".into(),
                reason: if use_inset {
                    "No honeycomb fits inside the inset at this grid position. Reduce the inset width or hexagon size, or adjust the grid offsets.".into()
                } else if args.include_partial_cells {
                    "No hexagon edges intersect the selected outline. Increase the outline size or reduce the hexagon size.".into()
                } else {
                    "No complete hexagon fits at this grid position. Adjust the X/Y grid offsets, reduce the hexagon size, or enable Trim cells at boundary.".into()
                },
            }));
        }
        let drawing_calls =
            outlines.len() + pattern.complete_cells.len() + pattern.clipped_paths.len();
        let max_drawing_calls = HOST_CALL_LIMIT - SETUP_CALLS;
        if drawing_calls > max_drawing_calls {
            return Err(selection_error(&format!(
                "This pattern needs {drawing_calls} sketch paths, but SolidCraft allows {max_drawing_calls} per run. Increase the hexagon size or divide the area into smaller runs. Turning off Trim cells at boundary can also reduce the path count."
            )));
        }
        let spare_calls = max_drawing_calls - drawing_calls;
        let clipped_edge_count = pattern
            .clipped_paths
            .iter()
            .map(|path| path.len() - 1)
            .sum::<usize>();

        // Commands run in one scratch transaction. Keep the user's sketch open;
        // no sketch is created, finished, or extruded by this generator.
        let existing: ParameterList =
            serde_json::from_value(run("parameters.list", json!({"filter":"all"}))?)
                .map_err(|e| PluginError::Trap(format!("Invalid parameters.list response: {e}")))?;
        let names = ParameterNames::allocate(&existing);
        let group = format!("Honeycomb {}", names.index);
        add_parameter(
            &names.size,
            args.across_flats_mm,
            &format!(
                "Hexagon size across flats for {group} in sketch {:?}.",
                boundary.sketch_name
            ),
        )?;
        add_parameter(
            &names.gap,
            args.gap_mm,
            &format!(
                "Edge-to-edge cell gap for {group} in sketch {:?}.",
                boundary.sketch_name
            ),
        )?;

        if spare_calls >= 3 {
            control::progress(0.15, "Adding honeycomb to the open sketch");
        }
        for outline in &outlines {
            check_cancelled()?;
            let points: Vec<_> = outline.iter().copied().map(fixed_point).collect();
            run(
                "sketch.line",
                json!({"points": points, "closed": true, "infer": false, "connect": false}),
            )?;
        }

        for cell in &pattern.complete_cells {
            check_cancelled()?;
            let points = cell.vertices.map(|point| {
                [
                    expression(
                        anchor.x,
                        point.x_pitch,
                        point.x_size,
                        &names.size,
                        &names.gap,
                    ),
                    expression(
                        anchor.y,
                        point.y_pitch,
                        point.y_size,
                        &names.size,
                        &names.gap,
                    ),
                ]
            });
            run(
                "sketch.line",
                json!({
                    "points":points, "closed":true, "infer":false, "connect":false
                }),
            )?;
        }
        for path in &pattern.clipped_paths {
            check_cancelled()?;
            let points: Vec<_> = path.iter().copied().map(fixed_point).collect();
            run(
                "sketch.line",
                json!({
                    "points":points,
                    "closed":false, "infer":false, "connect":false
                }),
            )?;
        }
        check_cancelled()?;

        let editable = json!({"across_flats_mm":names.size, "gap_mm":names.gap});
        if spare_calls >= 1 {
            log::log(log::Level::Info, &format!(
            "Added {} complete hexagons and {} clipped boundary edges to open sketch {:?} (ID {:?}). Editable parameters: {}",
                pattern.complete_cells.len(), clipped_edge_count, boundary.sketch_name, boundary.sketch_id, editable
            ));
        }
        if spare_calls >= 2 {
            control::progress(1.0, "Added honeycomb to the open sketch");
        }
        Ok(json!({
            "status":"created",
            "cells":pattern.complete_cells.len(),
            "clipped_edges":clipped_edge_count,
            "sketch":boundary.sketch_name,
            "sketch_id":boundary.sketch_id,
            "boundary_curves":boundary.polygons.iter().map(Vec::len).sum::<usize>(),
            "regions":boundary.polygons.len(),
            "boundary_inset_mm":inset,
            "inset_outlines":outlines.len(),
            "parameters":editable,
            "note":if clipped_edge_count == 0 { "The selected boundary remains in the open sketch. Edit size and gap in Solid > Modify > Change Parameters." } else { "Cut-off boundary edges are positioned for the size and gap used during generation; rerun the generator after changing those parameters." }
        }).to_string())
    }
}

fn fixed_point(point: Point) -> [String; 2] {
    [format!("{:.15} mm", point.x), format!("{:.15} mm", point.y)]
}

fn expression(origin: f64, pitch_factor: f64, size_factor: f64, size: &str, gap: &str) -> String {
    format!("{origin:.15} mm {pitch_factor:+.15} * ({size} + {gap}) {size_factor:+.15} * {size}")
}

fn read_boundary(anchor_mode: AnchorMode) -> Result<Boundary, PluginError> {
    let selected = run("selection.get", json!({"detail":true}))?;
    let items = selected["items"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut sketch_id = None;
    let mut curve_ids = Vec::new();
    let mut profile_indices = Vec::new();
    let mut selected_point_refs = Vec::new();
    for item in items {
        let kind = item["kind"]
            .as_str()
            .or_else(|| item["type"].as_str())
            .unwrap_or("");
        let entity_kind = item["entity"]["kind"]
            .as_str()
            .or_else(|| item["entity"]["type"].as_str())
            .unwrap_or("");
        if kind.contains("point") || entity_kind.contains("point") {
            let id = item["id"]
                .as_str()
                .or_else(|| item["point"].as_str())
                .or_else(|| item["entity"]["id"].as_str())
                .map(str::to_owned);
            let point = [
                &item["position"],
                &item["at"],
                &item["point"],
                &item["coordinates"],
                &item["entity"]["position"],
                &item["entity"]["point"],
            ]
            .into_iter()
            .find(|value| coordinate(value).is_some());
            selected_point_refs.push((id, point));
            if let Some(id_value) = item["sketch"]
                .as_u64()
                .or_else(|| item["sketch"]["id"].as_u64())
            {
                if sketch_id.is_some_and(|old| old != id_value) {
                    return Err(selection_error(
                        "Select an anchor point from the same sketch as the boundary.",
                    ));
                }
                sketch_id = Some(id_value);
            }
            continue;
        }
        if kind == "profile" {
            let this_sketch = item["sketch"]
                .as_u64()
                .or_else(|| item["sketch"]["id"].as_u64());
            if let Some(id_value) = this_sketch {
                if sketch_id.is_some_and(|old| old != id_value) {
                    return Err(selection_error(
                        "Select a profile from the active sketch only.",
                    ));
                }
                sketch_id = Some(id_value);
                let index = item["index"].as_u64().ok_or_else(|| {
                    selection_error("Could not read the selected sketch profile.")
                })?;
                if !profile_indices.contains(&index) {
                    profile_indices.push(index);
                }
            }
            continue;
        }
        if kind != "sketch_curve" {
            continue;
        }
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        let this_sketch = item["sketch"]
            .as_u64()
            .or_else(|| item["sketch"]["id"].as_u64());
        if let Some(id_value) = this_sketch {
            if sketch_id.is_some_and(|old| old != id_value) {
                return Err(selection_error(
                    "Select boundary lines from one sketch only.",
                ));
            }
            sketch_id = Some(id_value);
            curve_ids.push(id.to_owned());
        }
    }
    let sketch = run("sketch.inspect", json!({})).map_err(|error| match error {
        PluginError::CommandFailed(_) => {
            selection_error("Open a sketch before running the honeycomb generator.")
        }
        error => error,
    })?;
    let active_sketch_id = sketch["id"]
        .as_u64()
        .or_else(|| sketch["sketch_id"].as_u64())
        .or_else(|| sketch["sketch"]["id"].as_u64());
    if sketch_id.is_some() && sketch_id != active_sketch_id {
        return Err(selection_error(
            "Select profiles, boundary lines, and anchor points from the active sketch only.",
        ));
    }
    let sketch_name = sketch["name"]
        .as_str()
        .or_else(|| sketch["sketch_name"].as_str())
        .unwrap_or("Active Sketch")
        .to_owned();
    let points = point_positions(&sketch);
    let selected_anchor = if anchor_mode == AnchorMode::SelectedPoint {
        if selected_point_refs.len() != 1 {
            return Err(selection_error("Select exactly one sketch point to use as the pattern anchor, then run the generator."));
        }
        let (id, direct) = &selected_point_refs[0];
        id.as_ref().and_then(|id| points.get(id).copied())
            .or_else(|| direct.and_then(|position| sketch_position(position, &sketch["plane"])))
            .ok_or_else(|| selection_error("Could not read the selected sketch point. Select a sketch point, not a line or profile."))
            .map(Some)?
    } else {
        None
    };
    let curves = sketch["curves"]
        .as_array()
        .ok_or_else(|| PluginError::Trap("sketch.inspect did not return sketch curves".into()))?;
    let all_segments = collect_segments(curves, &points, None)?;
    let loops = closed_line_loops(all_segments);
    let mut polygons = Vec::new();
    if !profile_indices.is_empty() {
        let profiles = sketch["profiles"].as_array().ok_or_else(|| {
            PluginError::Trap("sketch.inspect did not return sketch profiles".into())
        })?;
        for index in profile_indices {
            let profile = profiles.iter().find(|profile| profile["index"].as_u64() == Some(index))
                .or_else(|| profiles.get(index as usize)).ok_or_else(|| {
                    selection_error("A selected sketch profile was not found; reselect the regions and try again.")
                })?;
            let polygon = profile_polygon(profile, curves, &points, &loops)?;
            if !polygons.contains(&polygon) {
                polygons.push(polygon);
            }
        }
    }
    if !curve_ids.is_empty() {
        let selected: HashSet<&str> = curve_ids.iter().map(String::as_str).collect();
        let segments = collect_segments(curves, &points, Some(&selected))?;
        if segments.len() != selected.len() {
            return Err(selection_error("Some selected boundary lines could not be read. Select all straight lines around each outline."));
        }
        let selected_loops = closed_line_loops(segments);
        if selected_loops
            .iter()
            .map(|outline| outline.curve_ids.len())
            .sum::<usize>()
            != selected.len()
        {
            return Err(selection_error(
                "Selected lines must form one or more complete closed outlines.",
            ));
        }
        for outline in selected_loops {
            polygons.push(outline.polygon);
        }
    }

    if polygons.is_empty() {
        if loops.len() == 1 {
            polygons.push(loops.into_iter().next().unwrap().polygon);
        } else if loops.len() > 1 {
            return Err(selection_error("Close Honeycomb and end any active sketch tool with Esc. Shift-click the regions to fill (or select all their boundary lines), then reopen Honeycomb."));
        }
    }
    if polygons.is_empty() {
        return Err(selection_error(
            "Could not find a closed straight-line outline in the active sketch.",
        ));
    }
    Ok(Boundary {
        sketch_id: active_sketch_id,
        sketch_name,
        polygons,
        selected_anchor,
    })
}

fn profile_polygon(
    profile: &Value,
    curves: &[Value],
    points: &HashMap<String, Point>,
    loops: &[BoundaryLoop],
) -> Result<Vec<Point>, PluginError> {
    let unmapped = || {
        selection_error("Could not read the selected profile as a closed straight-line outline. Try selecting its boundary lines.")
    };
    if profile["holes"]
        .as_array()
        .is_some_and(|holes| !holes.is_empty())
    {
        return Err(selection_error("Select a profile without holes."));
    }

    let area = ["area_mm2", "area", "profile_area_mm2"]
        .iter()
        .find_map(|key| profile[*key].as_f64())
        .map(f64::abs);
    let area_matches = |polygon: &[Point], area: f64| {
        (polygon_area(polygon).abs() - area).abs() <= area.max(1.0) * 1e-6
    };
    if let Some(profile_curves) = profile["curves"].as_array().filter(|ids| !ids.is_empty()) {
        // Each profile owns a separate loop. The same divider can appear in
        // two profiles, so ordering all connected sketch lines together fails.
        let ids: HashSet<&str> = profile_curves
            .iter()
            .map(Value::as_str)
            .collect::<Option<_>>()
            .ok_or_else(unmapped)?;
        let segments = collect_segments(curves, points, Some(&ids))?;
        if segments.len() < 3 || segments.len() != ids.len() {
            return Err(unmapped());
        }
        let polygon = order_closed_loop(segments).map_err(|_| unmapped())?;
        if !valid_polygon(&polygon) || area.is_some_and(|area| !area_matches(&polygon, area)) {
            return Err(unmapped());
        }
        return Ok(polygon);
    }

    let area = area.ok_or_else(unmapped)?;
    let mut matches = loops
        .iter()
        .filter(|outline| area_matches(&outline.polygon, area));
    let outline = matches.next().ok_or_else(unmapped)?;
    if matches.next().is_some() {
        return Err(unmapped());
    }
    Ok(outline.polygon.clone())
}

fn anchor_point(
    fill_area: &[Vec<Point>],
    selected_anchor: Option<Point>,
    args: &Parameters,
) -> Result<Point, PluginError> {
    let (min_x, min_y, max_x, max_y) = fill_area.iter().flatten().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |(min_x, min_y, max_x, max_y), p| {
            (
                min_x.min(p.x),
                min_y.min(p.y),
                max_x.max(p.x),
                max_y.max(p.y),
            )
        },
    );
    // At a corner, translate the cell inward by its rotated half-extents so
    // the hexagon's outermost points touch the corresponding boundary sides.
    let hexagon = geometry::unit_hexagon(args.orientation, args.rotation_deg);
    let half_width = hexagon
        .iter()
        .map(|point| point.x.abs())
        .fold(0.0, f64::max)
        * args.across_flats_mm;
    let half_height = hexagon
        .iter()
        .map(|point| point.y.abs())
        .fold(0.0, f64::max)
        * args.across_flats_mm;
    let base = match args.anchor_mode {
        AnchorMode::Center => Point { x: (min_x + max_x) / 2.0, y: (min_y + max_y) / 2.0 },
        AnchorMode::SelectedPoint => selected_anchor.ok_or_else(|| selection_error("Select exactly one sketch point before running the generator with Selected sketch point as the anchor."))?,
        AnchorMode::BottomLeft => Point { x: min_x + half_width, y: min_y + half_height },
        AnchorMode::BottomRight => Point { x: max_x - half_width, y: min_y + half_height },
        AnchorMode::TopLeft => Point { x: min_x + half_width, y: max_y - half_height },
        AnchorMode::TopRight => Point { x: max_x - half_width, y: max_y - half_height },
    };
    Ok(Point {
        x: base.x + args.offset_x_mm,
        y: base.y + args.offset_y_mm,
    })
}

fn collect_segments(
    curves: &[Value],
    points: &HashMap<String, Point>,
    selected: Option<&HashSet<&str>>,
) -> Result<Vec<Segment>, PluginError> {
    let mut segments = Vec::new();
    for curve in curves {
        let Some(id) = curve["id"].as_str() else {
            continue;
        };
        if selected.is_some_and(|ids| !ids.contains(id)) {
            continue;
        }
        let kind = curve["kind"]
            .as_str()
            .or_else(|| curve["type"].as_str())
            .or_else(|| curve["curve"].as_str())
            .unwrap_or("");
        if !kind.is_empty() && kind != "line" && kind != "Line" {
            if selected.is_some() {
                return Err(selection_error(
                    "Only straight sketch lines are supported for a boundary.",
                ));
            }
            continue;
        }
        if curve["construction"].as_bool() == Some(true) {
            continue;
        }
        if let Some((a, b)) = curve_endpoints(curve, points) {
            segments.push(Segment {
                id: id.to_owned(),
                a,
                b,
            });
        } else if selected.is_some() {
            return Err(PluginError::Trap(format!(
                "Could not read endpoints for selected boundary line {id}"
            )));
        }
    }
    Ok(segments)
}

fn closed_line_loops(segments: Vec<Segment>) -> Vec<BoundaryLoop> {
    let mut remaining = segments;
    let mut loops = Vec::new();
    while !remaining.is_empty() {
        let mut component = vec![remaining.remove(0)];
        loop {
            let endpoints: Vec<Point> = component
                .iter()
                .flat_map(|segment| [segment.a, segment.b])
                .collect();
            let Some(index) = remaining.iter().position(|segment| {
                endpoints
                    .iter()
                    .any(|point| near(*point, segment.a) || near(*point, segment.b))
            }) else {
                break;
            };
            component.push(remaining.remove(index));
        }
        let curve_ids = component.iter().map(|segment| segment.id.clone()).collect();
        if let Ok(polygon) = order_closed_loop(component) {
            if valid_polygon(&polygon) {
                loops.push(BoundaryLoop { polygon, curve_ids });
            }
        }
    }
    loops
}

fn valid_polygon(polygon: &[Point]) -> bool {
    polygon.len() >= 3 && polygon_area(polygon).abs() > 1e-8
}

fn point_positions(sketch: &Value) -> HashMap<String, Point> {
    let mut map = HashMap::new();
    if let Some(rows) = sketch["points"].as_array() {
        for row in rows {
            let id = row["id"].as_str().or_else(|| row["name"].as_str());
            let position = ["position", "at", "point", "coordinates", "xy"]
                .iter()
                .find_map(|key| coordinate(&row[*key]))
                .or_else(|| coordinate(row));
            if let (Some(id), Some(position)) = (id, position) {
                map.insert(id.to_owned(), position);
            }
        }
    }
    map
}

fn coordinate(value: &Value) -> Option<Point> {
    if let Some(pair) = value.as_array().filter(|pair| pair.len() >= 2) {
        return Some(Point {
            x: pair[0].as_f64()?,
            y: pair[1].as_f64()?,
        });
    }
    Some(Point {
        x: value["x"].as_f64()?,
        y: value["y"].as_f64()?,
    })
}

fn sketch_position(position: &Value, plane: &Value) -> Option<Point> {
    let point = coordinate(position)?;
    let Some(z) = position[2].as_f64().or_else(|| position["z"].as_f64()) else {
        return Some(point);
    };
    // selection.get reports world coordinates for sketch points and midpoints.
    let world = [point.x, point.y, z];
    let project = |axis: &Value| -> Option<f64> {
        let mut value = 0.0;
        for (index, coordinate) in world.iter().enumerate() {
            value += (coordinate - plane["origin"][index].as_f64()?) * axis[index].as_f64()?;
        }
        Some(value)
    };
    Some(Point {
        x: project(&plane["x_dir"])?,
        y: project(&plane["y_dir"])?,
    })
}

fn endpoint(curve: &Value, keys: &[&str], points: &HashMap<String, Point>) -> Option<Point> {
    for key in keys {
        let value = &curve[*key];
        if let Some(point) = coordinate(value) {
            return Some(point);
        }
        if let Some(id) = value.as_str().and_then(|id| points.get(id)) {
            return Some(*id);
        }
        if let Some(id) = value["id"].as_str().and_then(|id| points.get(id)) {
            return Some(*id);
        }
    }
    None
}

fn curve_endpoints(curve: &Value, points: &HashMap<String, Point>) -> Option<(Point, Point)> {
    if let Some(list) = curve["points"].as_array().filter(|list| list.len() == 2) {
        let point = |value: &Value| {
            coordinate(value)
                .or_else(|| value.as_str().and_then(|id| points.get(id).copied()))
                .or_else(|| value["id"].as_str().and_then(|id| points.get(id).copied()))
        };
        if let (Some(a), Some(b)) = (point(&list[0]), point(&list[1])) {
            return Some((a, b));
        }
    }
    Some((
        endpoint(curve, &["start", "a", "from", "p1"], points)?,
        endpoint(curve, &["end", "b", "to", "p2"], points)?,
    ))
}

fn order_closed_loop(mut segments: Vec<Segment>) -> Result<Vec<Point>, PluginError> {
    let first = segments.remove(0);
    let start = first.a;
    let mut current = first.b;
    let mut polygon = vec![start, current];
    while !segments.is_empty() {
        let Some(index) = segments
            .iter()
            .position(|segment| near(segment.a, current) || near(segment.b, current))
        else {
            return Err(selection_error(
                "The selected lines do not form one connected closed outline.",
            ));
        };
        let segment = segments.remove(index);
        current = if near(segment.a, current) {
            segment.b
        } else {
            segment.a
        };
        if near(current, start) {
            if !segments.is_empty() {
                return Err(selection_error(
                    "Select exactly one closed outline, without extra or nested boundary lines.",
                ));
            }
            break;
        }
        polygon.push(current);
    }
    if !near(current, start) {
        return Err(selection_error(
            "The selected lines do not form a closed outline.",
        ));
    }
    Ok(polygon)
}

fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).hypot(a.y - b.y) <= 1e-5
}

fn selection_error(reason: &str) -> PluginError {
    PluginError::InvalidInput(FieldFailure {
        field: "selection".into(),
        reason: reason.into(),
    })
}

fn add_parameter(name: &str, value: f64, comment: &str) -> Result<(), PluginError> {
    run(
        "parameters.add",
        json!({
            "name":name,
            "expression":format!("{value} mm"),
            "unit":"mm",
            "comment":comment,
        }),
    )?;
    Ok(())
}

fn check_cancelled() -> Result<(), PluginError> {
    if control::cancelled() {
        return Err(PluginError::Cancelled);
    }
    Ok(())
}

fn run(command: &str, args: Value) -> Result<Value, PluginError> {
    check_cancelled()?;
    let output = commands::run(command, &args.to_string())?;
    serde_json::from_str(&output).map_err(|e| {
        PluginError::Trap(format!(
            "SolidCraft returned invalid JSON for {command}: {e}"
        ))
    })
}

export!(HoneycombPlugin);
