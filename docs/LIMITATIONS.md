# Limitations and desktop verification

The plugin uses SolidCraft's experimental `craft:plugin@0.1.0` guest interface and `commands@0.1` capability. It calls `selection.get`, `sketch.inspect`, `parameters.list`, `parameters.add`, and `sketch.line`.

| Capability | Implementation |
| --- | --- |
| Boundary input | The user opens a sketch. A single closed line loop is detected automatically; when there are several loops, select all target profile regions or all boundary lines for each target region. |
| Boundary placement | The plugin applies one lattice across the union of selected polygons, including disconnected regions. The lattice can be anchored at the union center, a bounding-box corner, or a selected sketch point, with numeric X/Y offsets. An optional setting adds clipped edge segments at each selected outline. |
| Active sketch | `sketch.line` adds geometry to the sketch already being edited. The plugin does not create, finish, or extrude a sketch. |
| Editable dimensions | One numbered group adds size and gap parameters. Generated vertex expressions reference both parameters. |
| Transaction behavior | The host stages all commands in one scratch transaction. Errors and cancellation discard the whole invocation. |
| Work limit | The plugin rejects patterns over 250 cells before editing the sketch. Complete cells use one closed sketch path each. Each connected part of a clipped cell uses one open path. SolidCraft's host limit allows at most 251 drawing calls after setup. |

## Boundary requirements

SolidCraft's plugin dialog has no viewport selection field or hook for sketch tools and keyboard input. End the active tool with Esc and pick the regions before opening Honeycomb.

Each selected boundary must be a closed loop of at least three straight sketch lines. Arc, circle, spline, and body-edge boundaries are not supported. For multiple outlines, select all target profile regions, or all straight boundary segments for each target region, before opening the generator. The boundary is preserved, so users can delete its lines after generation if they do not want it included in the sketch profiles.

Complete hexagons remain expression-driven after creation. Clipped segments and any inset outline are fixed at generation. Rerun the generator to recalculate them after changing size or gap. Cut-off cells use the inset outline to close their profiles at a positive **Inset width**. At **0 mm**, they use the original sketch boundary. Rounded inset corners are approximated by tangent line segments.

The grid orientation, rotation, chosen cells, and placement are fixed at creation. The arrangement is a set of expression-driven sketch loops rather than a native associative rectangular-pattern feature. The plugin also reports an error if the selected area spans too many grid positions to search in one run.

## Desktop verification

Install the built ZIP in the target SolidCraft desktop release. Open a sketch and select **Run** from the Honeycomb dialog for each case.

- Fill a rectangle, a concave polygon, and multiple selected regions. Check that cells stay within the fill area and share one grid across separate regions. Confirm that selections from another sketch are rejected.
- Run with boundary trimming and an inset. Check that the original boundary remains, the inset is drawn, and clipped edges follow the fill boundary.
- Confirm that the sketch stays open. Verify that one undo removes the whole generation.
- Generate the pattern again and change its size and gap parameters. Confirm that complete cells update and that clipped edges and the inset outline stay fixed.

## References

- [SolidCraft command reference](https://docs.solidcraft.io/reference/commands.html)
- [SolidCraft control protocol](https://docs.solidcraft.io/dev/control-protocol.html)
- [Plugin runtime and author guide](https://github.com/tangency-labs/solidcraft/blob/v0.41.0/docs/plugin-runtime.md)
- [Plugin WIT](https://github.com/tangency-labs/solidcraft/blob/v0.41.0/crates/craft-plugin/wit/plugin.wit)
