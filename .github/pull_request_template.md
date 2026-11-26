Describe the change and how you tested it.

## 3D rendering evidence

<!-- For runtime, shader, build, or rendering-tool changes, compare the same
scene and camera on the same adapter, driver, settings and cache state.
Check geometry, textures, lighting, presentation and frame pacing. A visible
regression blocks the PR. The gate also rejects an increase of more than 10%
in either median or p95 frame time on the matched workload. Include
before/after images or a readable comparison report. Synthetic scenes are
welcome. Keep game assets, saves, shaders and proprietary captures out of Git.
Update the evidence when the tested commit changes. Documentation-only PRs
can omit this section. -->

Baseline commit: <!-- current base SHA -->
Candidate commit: <!-- PR head SHA -->
Scene and settings: <!-- scene, camera, adapter, driver, profile and cache state -->
Visual comparison: <!-- HTTPS link to paired images or a comparison report -->
Frame times (median/p95 ms): <!-- baseline median/p95 -> candidate median/p95 -->

- [ ] I verified that this change does not regress 3D rendering or frame pacing.
