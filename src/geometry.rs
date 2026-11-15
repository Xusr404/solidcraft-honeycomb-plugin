use serde::Deserialize;
use std::f64::consts::PI;

mod inset;
pub use inset::inset_outlines;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ExpressionPoint {
    pub x_pitch: f64,
    pub x_size: f64,
    pub y_pitch: f64,
    pub y_size: f64,
}

#[derive(Debug)]
pub struct BoundaryCell {
    pub vertices: [ExpressionPoint; 6],
}

#[derive(Clone, Copy, Debug)]
pub struct BoundarySegment {
    pub a: Point,
    pub b: Point,
}

#[derive(Debug)]
pub struct BoundaryPattern {
    pub complete_cells: Vec<BoundaryCell>,
    pub clipped_paths: Vec<Vec<Point>>,
}

pub const MAX_CELLS: usize = 250;

#[derive(Debug)]
pub enum PatternLimit {
    CellCount,
    SearchArea,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    #[serde(rename = "Flat top", alias = "flat-top")]
    FlatTop,
    #[serde(rename = "Pointy top", alias = "pointy-top")]
    PointyTop,
}

/// Offsets from a cell center per millimetre of across-flats size.
pub fn unit_hexagon(orientation: Orientation, rotation_deg: f64) -> [Point; 6] {
    let angle0 = if orientation == Orientation::FlatTop {
        0.0
    } else {
        PI / 6.0
    };
    let rotation = rotation_deg.to_radians();
    let radius = 1.0 / 3.0_f64.sqrt();
    std::array::from_fn(|index| {
        let angle = angle0 + rotation + index as f64 * PI / 3.0;
        let (sin, cos) = angle.sin_cos();
        Point {
            x: radius * cos,
            y: radius * sin,
        }
    })
}

struct FillRegion<'a> {
    polygons: &'a [Vec<Point>],
    edges: Vec<BoundarySegment>,
    use_even_odd: bool,
}

impl<'a> FillRegion<'a> {
    fn new(polygons: &'a [Vec<Point>], use_even_odd: bool) -> Self {
        let edges = polygons
            .iter()
            .flat_map(|polygon| polygon_edges(polygon))
            .collect();
        Self {
            polygons,
            edges,
            use_even_odd,
        }
    }

    fn contains(&self, point: Point) -> bool {
        if self.use_even_odd {
            self.edges
                .iter()
                .any(|edge| point_on_segment(point, edge.a, edge.b))
                || self
                    .polygons
                    .iter()
                    .filter(|polygon| point_in_polygon(point, polygon))
                    .count()
                    % 2
                    == 1
        } else {
            point_in_regions(point, self.polygons)
        }
    }
}

/// Find cells in the union of the supplied polygons, or reject a pattern
/// that exceeds the work limits. The origin sets the grid alignment.
pub fn pattern_in_boundary(
    polygons: &[Vec<Point>],
    origin: Point,
    orientation: Orientation,
    size: f64,
    gap: f64,
    rotation_deg: f64,
    include_partial_cells: bool,
    use_even_odd: bool,
) -> Result<BoundaryPattern, PatternLimit> {
    let (min_x, min_y, max_x, max_y) = polygons.iter().flatten().fold(
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
    let width = max_x - min_x;
    let height = max_y - min_y;
    let pitch = size + gap;
    let radius = size / 3.0_f64.sqrt();
    let flat = orientation == Orientation::FlatTop;
    let stagger = 1.5 / 3.0_f64.sqrt();
    let (step_x, step_y) = if flat { (stagger, 1.0) } else { (1.0, stagger) };

    let hexagon = unit_hexagon(orientation, rotation_deg);
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    // Find the boundary center in the rotated grid's coordinate system. Keep
    // the original origin and row/column parity when evaluating each cell.
    let dx = min_x + width / 2.0 - origin.x;
    let dy = min_y + height / 2.0 - origin.y;
    let col = ((dx * cos + dy * sin) / (step_x * pitch)).round();
    let row = ((-dx * sin + dy * cos) / (step_y * pitch)).round();
    let x_extent = (width * cos.abs() + height * sin.abs()) / 2.0 + radius;
    let y_extent = (width * sin.abs() + height * cos.abs()) / 2.0 + radius;
    let nx = (x_extent / (step_x * pitch)).ceil() + 1.0;
    let ny = (y_extent / (step_y * pitch)).ceil() + 1.0;
    if !col.is_finite() || !row.is_finite() || !(nx * ny <= 100_000.0) {
        return Err(PatternLimit::SearchArea);
    }
    let (nx, ny) = (nx as i32, ny as i32);
    let region = FillRegion::new(polygons, use_even_odd);
    let mut candidates = Vec::new();
    for col_offset in -nx..=nx {
        let i = col + f64::from(col_offset);
        for row_offset in -ny..=ny {
            let j = row + f64::from(row_offset);
            let (cx_pitch, cy_pitch) = if flat {
                (i * step_x, (j + i.rem_euclid(2.0) * 0.5) * step_y)
            } else {
                ((i + j.rem_euclid(2.0) * 0.5) * step_x, j * step_y)
            };
            let (px, py) = (
                cx_pitch * cos - cy_pitch * sin,
                cx_pitch * sin + cy_pitch * cos,
            );
            let center = Point {
                x: origin.x + px * pitch,
                y: origin.y + py * pitch,
            };
            let vertices = hexagon.map(|vertex| ExpressionPoint {
                x_pitch: px,
                x_size: vertex.x,
                y_pitch: py,
                y_size: vertex.y,
            });
            let actual: [Point; 6] = std::array::from_fn(|k| Point {
                x: center.x + vertices[k].x_size * size,
                y: center.y + vertices[k].y_size * size,
            });
            let wholly_inside = actual.iter().all(|point| region.contains(*point))
                && (0..6).all(|k| {
                    let a = actual[k];
                    let b = actual[(k + 1) % 6];
                    let segments = clip_segment_to_region(a, b, &region);
                    segments.len() == 1 && near(segments[0].a, a) && near(segments[0].b, b)
                })
                && (!use_even_odd
                    || region
                        .edges
                        .iter()
                        .all(|edge| !point_in_polygon(edge.a, &actual)));
            if wholly_inside {
                let distance = (center.x - origin.x).powi(2) + (center.y - origin.y).powi(2);
                candidates.push((distance, Some(vertices), Vec::new()));
            } else if include_partial_cells {
                let mut clipped = Vec::new();
                for k in 0..6 {
                    clipped.extend(clip_segment_to_region(
                        actual[k],
                        actual[(k + 1) % 6],
                        &region,
                    ));
                }
                if !clipped.is_empty() {
                    let distance = (center.x - origin.x).powi(2) + (center.y - origin.y).powi(2);
                    candidates.push((distance, None, clipped_paths(clipped)));
                }
            }
            if candidates.len() > MAX_CELLS {
                return Err(PatternLimit::CellCount);
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut pattern = BoundaryPattern {
        complete_cells: Vec::new(),
        clipped_paths: Vec::new(),
    };
    for (_, vertices, clipped) in candidates {
        if let Some(vertices) = vertices {
            pattern.complete_cells.push(BoundaryCell { vertices });
        }
        pattern.clipped_paths.extend(clipped);
    }
    Ok(pattern)
}

// Segments arrive in perimeter order. The first and last paths can also join
// when a trimmed cell crosses the perimeter's starting vertex.
fn clipped_paths(segments: Vec<BoundarySegment>) -> Vec<Vec<Point>> {
    let mut paths: Vec<Vec<Point>> = Vec::new();
    for segment in segments {
        if let Some(path) = paths
            .last_mut()
            .filter(|path| near(path[path.len() - 1], segment.a))
        {
            path.push(segment.b);
        } else {
            paths.push(vec![segment.a, segment.b]);
        }
    }
    if paths.len() > 1 && near(paths[0][0], *paths.last().unwrap().last().unwrap()) {
        let mut last = paths.pop().unwrap();
        last.extend(paths[0].iter().skip(1));
        paths[0] = last;
    }
    paths
}

fn clip_segment_to_region(a: Point, b: Point, region: &FillRegion<'_>) -> Vec<BoundarySegment> {
    let direction = delta(a, b);
    if dot(direction, direction) <= 1e-16 {
        return Vec::new();
    }
    let mut cuts = vec![0.0, 1.0];
    for edge in &region.edges {
        cuts.extend(segment_intersection_parameters(a, b, edge.a, edge.b));
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
    let mut intervals: Vec<(f64, f64)> = Vec::new();
    for pair in cuts.windows(2) {
        let (t0, t1) = (pair[0], pair[1]);
        if t1 - t0 <= 1e-9 {
            continue;
        }
        if region.contains(at(a, direction, (t0 + t1) / 2.0)) {
            if let Some(last) = intervals
                .last_mut()
                .filter(|last| (last.1 - t0).abs() <= 1e-9)
            {
                last.1 = t1;
            } else {
                intervals.push((t0, t1));
            }
        }
    }
    intervals
        .into_iter()
        .map(|(t0, t1)| BoundarySegment {
            a: at(a, direction, t0),
            b: at(a, direction, t1),
        })
        .collect()
}

// A segment's inset exclusion zone is a strip capped by two circles.
// Extra cuts on the extensions of these curves are harmless. Midpoint
// classification determines which intervals actually meet the clearance.
fn add_inset_cuts(
    cuts: &mut Vec<f64>,
    a: Point,
    direction: Point,
    edge: BoundarySegment,
    inset: f64,
) {
    let mut add = |t: f64| {
        if (0.0..1.0).contains(&t) {
            cuts.push(t);
        }
    };
    let side = delta(edge.a, edge.b);
    let rate = cross(side, direction);
    if rate.abs() > 1e-12 {
        let start = cross(side, delta(edge.a, a));
        let offset = inset * dot(side, side).sqrt();
        add((offset - start) / rate);
        add((-offset - start) / rate);
    }
    let length_squared = dot(direction, direction);
    for endpoint in [edge.a, edge.b] {
        let rel = delta(a, endpoint);
        let center = dot(rel, direction) / length_squared;
        let distance = cross(direction, rel) / length_squared.sqrt();
        let discriminant = inset * inset - distance * distance;
        if discriminant >= 0.0 {
            let half = (discriminant / length_squared).sqrt();
            add(center - half);
            add(center + half);
        }
    }
}

fn polygon_edges(polygon: &[Point]) -> impl Iterator<Item = BoundarySegment> + '_ {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .filter_map(|(&a, &b)| (!near(a, b)).then_some(BoundarySegment { a, b }))
}

pub fn polygon_area(points: &[Point]) -> f64 {
    let Some(&origin) = points.first() else {
        return 0.0;
    };
    polygon_edges(points)
        .map(|edge| cross(delta(origin, edge.a), delta(origin, edge.b)))
        .sum::<f64>()
        / 2.0
}

fn union_boundary(polygons: &[Vec<Point>]) -> Vec<BoundarySegment> {
    let edges: Vec<Vec<_>> = polygons
        .iter()
        .map(|polygon| polygon_edges(polygon).collect())
        .collect();
    let winding: Vec<_> = polygons
        .iter()
        .map(|polygon| polygon_area(polygon).signum())
        .collect();
    let mut exposed = Vec::new();
    for (region, boundary) in edges.iter().enumerate() {
        for edge in boundary {
            let direction = delta(edge.a, edge.b);
            let mut cuts = vec![0.0, 1.0];
            for other in edges
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != region)
                .flat_map(|(_, edges)| edges)
            {
                cuts.extend(segment_intersection_parameters(
                    edge.a, edge.b, other.a, other.b,
                ));
            }
            cuts.sort_by(f64::total_cmp);
            cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            for cut in cuts.windows(2) {
                let middle = at(edge.a, direction, (cut[0] + cut[1]) / 2.0);
                let covered = polygons.iter().enumerate().any(|(index, polygon)| {
                    if index == region {
                        return false;
                    }
                    if let Some(other) = edges[index]
                        .iter()
                        .find(|other| point_on_segment(middle, other.a, other.b))
                    {
                        // Coincident edges with interiors on opposite sides are internal seams.
                        return winding[region]
                            * winding[index]
                            * dot(direction, delta(other.a, other.b))
                            < 0.0;
                    }
                    point_in_polygon(middle, polygon)
                });
                if !covered {
                    let a = at(edge.a, direction, cut[0]);
                    let b = at(edge.a, direction, cut[1]);
                    if !near(a, b) {
                        exposed.push(if winding[region] > 0.0 {
                            BoundarySegment { a, b }
                        } else {
                            BoundarySegment { a: b, b: a }
                        });
                    }
                }
            }
        }
    }
    exposed
}

fn delta(a: Point, b: Point) -> Point {
    Point {
        x: b.x - a.x,
        y: b.y - a.y,
    }
}
fn at(a: Point, direction: Point, t: f64) -> Point {
    Point {
        x: a.x + direction.x * t,
        y: a.y + direction.y * t,
    }
}
fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}
fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}

fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).hypot(a.y - b.y) <= 1e-7
}

fn point_in_regions(point: Point, polygons: &[Vec<Point>]) -> bool {
    polygons
        .iter()
        .any(|polygon| point_in_or_on_polygon(point, polygon))
}

fn segment_intersection_parameters(a: Point, b: Point, c: Point, d: Point) -> Vec<f64> {
    let (rx, ry, sx, sy) = (b.x - a.x, b.y - a.y, d.x - c.x, d.y - c.y);
    let cross = rx * sy - ry * sx;
    let qpx = c.x - a.x;
    let qpy = c.y - a.y;
    if cross.abs() <= 1e-12 {
        if (qpx * ry - qpy * rx).abs() > 1e-9 {
            return Vec::new();
        }
        let denominator = rx * rx + ry * ry;
        return [c, d]
            .into_iter()
            .filter_map(|point| {
                let t = ((point.x - a.x) * rx + (point.y - a.y) * ry) / denominator;
                (-1e-9..=1.0 + 1e-9)
                    .contains(&t)
                    .then_some(t.clamp(0.0, 1.0))
            })
            .collect();
    }
    let t = (qpx * sy - qpy * sx) / cross;
    let u = (qpx * ry - qpy * rx) / cross;
    if (-1e-9..=1.0 + 1e-9).contains(&t) && (-1e-9..=1.0 + 1e-9).contains(&u) {
        vec![t.clamp(0.0, 1.0)]
    } else {
        Vec::new()
    }
}

fn point_in_or_on_polygon(point: Point, polygon: &[Point]) -> bool {
    (0..polygon.len())
        .any(|i| point_on_segment(point, polygon[i], polygon[(i + 1) % polygon.len()]))
        || point_in_polygon(point, polygon)
}

fn point_on_segment(point: Point, a: Point, b: Point) -> bool {
    let cross = (point.x - a.x) * (b.y - a.y) - (point.y - a.y) * (b.x - a.x);
    if cross.abs() > 1e-8 {
        return false;
    }
    let dot = (point.x - a.x) * (b.x - a.x) + (point.y - a.y) * (b.y - a.y);
    dot >= -1e-8 && dot <= (b.x - a.x).powi(2) + (b.y - a.y).powi(2) + 1e-8
}

fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[j];
        if (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_cells_follow_offsets_with_or_without_trimming() {
        let region = [vec![
            Point { x: -30.0, y: -25.0 },
            Point { x: 30.0, y: -25.0 },
            Point { x: 30.0, y: 25.0 },
            Point { x: -30.0, y: 25.0 },
        ]];
        let size = 10.0;
        let gap = 0.5;
        let rotation: f64 = 27.0;
        let (sin, cos) = rotation.to_radians().sin_cos();
        let offset = Point { x: 1.2, y: -0.8 };
        // A large translation by whole lattice periods preserves alignment.
        let periods = 1000.0 * (size + gap);
        let centers = |anchor: Point, pattern: &BoundaryPattern| -> Vec<Point> {
            pattern
                .complete_cells
                .iter()
                .map(|cell| Point {
                    x: anchor.x + cell.vertices[0].x_pitch * (size + gap),
                    y: anchor.y + cell.vertices[0].y_pitch * (size + gap),
                })
                .collect()
        };
        for (orientation, shift) in [
            (
                Orientation::FlatTop,
                Point {
                    x: -sin * periods,
                    y: cos * periods,
                },
            ),
            (
                Orientation::PointyTop,
                Point {
                    x: cos * periods,
                    y: sin * periods,
                },
            ),
        ] {
            let far = Point {
                x: offset.x + shift.x,
                y: offset.y + shift.y,
            };
            let trimmed = pattern_in_boundary(
                &region,
                offset,
                orientation,
                size,
                gap,
                rotation,
                true,
                false,
            )
            .unwrap();
            let complete =
                pattern_in_boundary(&region, far, orientation, size, gap, rotation, false, false)
                    .unwrap();
            let trimmed_centers = centers(offset, &trimmed);
            let complete_centers = centers(far, &complete);
            assert!(!trimmed.clipped_paths.is_empty());
            assert!(complete.clipped_paths.is_empty());
            assert!(trimmed_centers.iter().any(|&center| near(center, offset)));
            assert_eq!(trimmed_centers.len(), complete_centers.len());
            assert!(trimmed_centers.iter().all(|&center| {
                complete_centers
                    .iter()
                    .any(|&candidate| near(center, candidate))
            }));
        }
    }

    #[test]
    fn inset_clips_the_outer_edges_and_keeps_a_shared_seam() {
        let square = |left, right| {
            vec![
                Point { x: left, y: 0.0 },
                Point { x: right, y: 0.0 },
                Point { x: right, y: 10.0 },
                Point { x: left, y: 10.0 },
            ]
        };
        let polygons = [square(0.0, 10.0), square(10.0, 20.0), square(25.0, 35.0)];
        let outlines = inset_outlines(&polygons, 2.0).unwrap();
        assert_eq!(outlines.len(), 2);
        let region = FillRegion::new(&outlines, true);
        let clipped = clip_segment_to_region(
            Point { x: -2.0, y: 5.0 },
            Point { x: 37.0, y: 5.0 },
            &region,
        );
        assert_eq!(clipped.len(), 2);
        for (segment, (left, right)) in clipped.iter().zip([(2.0, 18.0), (27.0, 33.0)]) {
            assert!(near(segment.a, Point { x: left, y: 5.0 }));
            assert!(near(segment.b, Point { x: right, y: 5.0 }));
        }
    }

    #[test]
    fn inset_keeps_the_requested_clearance_around_a_concave_corner() {
        let polygons = [vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 5.0 },
            Point { x: 5.0, y: 5.0 },
            Point { x: 5.0, y: 10.0 },
            Point { x: 0.0, y: 10.0 },
        ]];
        let outlines = inset_outlines(&polygons, 2.0).unwrap();
        assert_eq!(outlines.len(), 1);
        let region = FillRegion::new(&outlines, true);
        let clipped =
            clip_segment_to_region(Point { x: 2.0, y: 4.0 }, Point { x: 8.0, y: 4.0 }, &region);
        assert_eq!(clipped.len(), 1);
        let exact = 5.0 - 3.0_f64.sqrt();
        assert!((exact - 0.01..=exact + 1e-7).contains(&clipped[0].b.x));
    }

    #[test]
    fn inset_can_split_a_region_at_a_narrow_neck() {
        let (sin, cos) = 23.0_f64.to_radians().sin_cos();
        let polygon = [
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 4.0),
            (16.0, 4.0),
            (16.0, 0.0),
            (26.0, 0.0),
            (26.0, 10.0),
            (16.0, 10.0),
            (16.0, 6.0),
            (10.0, 6.0),
            (10.0, 10.0),
            (0.0, 10.0),
        ]
        .map(|(x, y)| Point {
            x: x * cos - y * sin,
            y: x * sin + y * cos,
        })
        .to_vec();
        let outlines = inset_outlines(&[polygon.clone()], 2.0).unwrap();
        assert_eq!(outlines.len(), 2);
        assert!(outlines.iter().all(|outline| polygon_area(outline) > 36.0));
        assert!(inset_outlines(&[polygon], 6.0).unwrap().is_empty());
    }
}
