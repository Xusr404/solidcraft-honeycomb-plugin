use super::*;

/// Build the boundary of the selected union after removing the clearance
/// around each exposed edge. Concave corners are drawn with tangent facets.
pub fn inset_outlines(
    polygons: &[Vec<Point>],
    inset: f64,
) -> Result<Vec<Vec<Point>>, &'static str> {
    let edges = union_boundary(polygons);
    let mut vertices = Vec::new();
    for point in edges.iter().flat_map(|edge| [edge.a, edge.b]) {
        if !vertices.iter().any(|&vertex| near(vertex, point)) {
            vertices.push(point);
        }
    }

    let contains = |point: Point| {
        point_in_regions(point, polygons)
            && edges.iter().all(|edge| {
                let direction = delta(edge.a, edge.b);
                let t = (dot(delta(edge.a, point), direction) / dot(direction, direction))
                    .clamp(0.0, 1.0);
                let distance = delta(at(edge.a, direction, t), point);
                dot(distance, distance) >= (inset - 1e-8).powi(2)
            })
    };
    let probe = (inset * 1e-4).min(1e-6);
    let mut pieces = Vec::new();
    for edge in &edges {
        let direction = delta(edge.a, edge.b);
        let normal = left_unit(direction);
        let start = at(edge.a, normal, inset);
        let mut cuts = vec![0.0, 1.0];
        for other in &edges {
            add_inset_cuts(&mut cuts, start, direction, *other, inset);
        }
        sort_cuts(&mut cuts);
        for pair in cuts.windows(2) {
            let middle = at(start, direction, (pair[0] + pair[1]) / 2.0);
            if contains(middle) && contains(at(middle, normal, probe)) {
                append_piece(
                    &mut pieces,
                    at(start, direction, pair[0]),
                    at(start, direction, pair[1]),
                );
            }
        }
    }

    for &center in &vertices {
        let mut cuts = vec![0.0, 2.0 * PI];
        for &other in &vertices {
            let direction = delta(center, other);
            let distance = direction.x.hypot(direction.y);
            if distance > 1e-7 && distance <= 2.0 * inset {
                add_angles(
                    &mut cuts,
                    direction.y.atan2(direction.x),
                    distance / (2.0 * inset),
                );
            }
        }
        for edge in &edges {
            let normal = left_unit(delta(edge.a, edge.b));
            let angle = normal.y.atan2(normal.x);
            let distance = dot(delta(center, edge.a), normal);
            add_angles(&mut cuts, angle, (distance + inset) / inset);
            add_angles(&mut cuts, angle, (distance - inset) / inset);
        }
        sort_cuts(&mut cuts);
        for pair in cuts.windows(2) {
            let direction = polar((pair[0] + pair[1]) / 2.0);
            if contains(at(center, direction, inset))
                && contains(at(center, direction, inset + probe))
            {
                append_arc(&mut pieces, center, inset, pair[1], pair[0]);
            }
        }
    }

    trace_outlines(pieces)
}

fn left_unit(direction: Point) -> Point {
    let length = direction.x.hypot(direction.y);
    Point {
        x: -direction.y / length,
        y: direction.x / length,
    }
}

fn polar(angle: f64) -> Point {
    Point {
        x: angle.cos(),
        y: angle.sin(),
    }
}

fn add_angles(cuts: &mut Vec<f64>, angle: f64, cosine: f64) {
    if cosine.abs() <= 1.0 + 1e-12 {
        let delta = cosine.clamp(-1.0, 1.0).acos();
        cuts.extend([
            (angle - delta).rem_euclid(2.0 * PI),
            (angle + delta).rem_euclid(2.0 * PI),
        ]);
    }
}

fn sort_cuts(cuts: &mut Vec<f64>) {
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
}

fn append_piece(pieces: &mut Vec<BoundarySegment>, a: Point, b: Point) {
    if !near(a, b) && !pieces.iter().any(|edge| near(edge.a, a) && near(edge.b, b)) {
        pieces.push(BoundarySegment { a, b });
    }
}

fn append_arc(pieces: &mut Vec<BoundarySegment>, center: Point, radius: f64, start: f64, end: f64) {
    // Tangent facets preserve the minimum border width around the vertex.
    let segments = ((start - end) / (PI / 18.0)).ceil() as u32;
    let step = (end - start) / f64::from(segments);
    let mut previous = at(center, polar(start), radius);
    for index in 0..segments {
        let angle = start + (f64::from(index) + 0.5) * step;
        let corner = at(center, polar(angle), radius / (step / 2.0).cos());
        append_piece(pieces, previous, corner);
        previous = corner;
    }
    append_piece(pieces, previous, at(center, polar(end), radius));
}

fn trace_outlines(mut pieces: Vec<BoundarySegment>) -> Result<Vec<Vec<Point>>, &'static str> {
    let mut outlines = Vec::new();
    while let Some(first) = pieces.pop() {
        let mut outline = vec![first.a];
        let mut edge = first;
        while !near(edge.b, first.a) {
            outline.push(edge.b);
            let incoming = delta(edge.a, edge.b);
            let turn = |next: &BoundarySegment| {
                let outgoing = delta(next.a, next.b);
                cross(incoming, outgoing).atan2(dot(incoming, outgoing))
            };
            let index = pieces.iter().enumerate().filter(|(_, next)| near(next.a, edge.b))
                .max_by(|(_, a), (_, b)| turn(a).total_cmp(&turn(b)))
                .map(|(index, _)| index).ok_or("Could not close the offset outline at this border width. Try a different border width.")?;
            edge = pieces.swap_remove(index);
        }
        // Remove subdivisions introduced by clearance intersections along the same line.
        let outline: Vec<_> = (0..outline.len())
            .filter_map(|index| {
                let previous = outline[(index + outline.len() - 1) % outline.len()];
                let point = outline[index];
                let next = outline[(index + 1) % outline.len()];
                let incoming = delta(previous, point);
                let outgoing = delta(point, next);
                let collinear = cross(incoming, outgoing).abs()
                    <= 1e-9 * incoming.x.hypot(incoming.y) * outgoing.x.hypot(outgoing.y);
                (!collinear || dot(incoming, outgoing) <= 0.0).then_some(point)
            })
            .collect();
        if outline.len() >= 3 && polygon_area(&outline).abs() > 1e-8 {
            outlines.push(outline);
        }
    }
    Ok(outlines)
}
