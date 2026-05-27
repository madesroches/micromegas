/**
 * GLSL patch for per-instance RGBA on a MeshBasicMaterial.
 *
 * Extracted from MapViewer.tsx (#1089). `THREE.InstancedMesh` only ships a
 * per-instance RGB color path (`instanceColor`); markers need per-instance
 * *alpha* too (the overlay encodes opacity per row). This injects a custom
 * `vec4 instanceColorRGBA` geometry attribute and routes it through to the
 * fragment output. Each block below is documented with the `#include <chunk>`
 * it overrides and why.
 */
import * as THREE from 'three'

// Vertex: declare the attribute + a varying alongside the standard uniforms.
// Overrides `#include <common>` (the first chunk, always present) because it is
// a stable anchor to append top-of-shader declarations without clobbering any
// real chunk body.
const VERTEX_COMMON = `#include <common>
attribute vec4 instanceColorRGBA;
varying vec4 vInstanceColor;`

// Vertex: capture the per-instance color into the varying once the position
// pipeline has started. Overrides `#include <begin_vertex>` purely as a
// convenient in-`main()` insertion point (its body is preserved above the
// appended line).
const VERTEX_BEGIN = `#include <begin_vertex>
vInstanceColor = instanceColorRGBA;`

// Fragment: declare the matching varying. Overrides the fragment-side
// `#include <common>` for the same reason as the vertex side.
const FRAGMENT_COMMON = `#include <common>
varying vec4 vInstanceColor;`

// Fragment: write the final color from the per-instance RGBA. Overrides
// `#include <opaque_fragment>` specifically — modern three.js (r152+) wraps the
// final `gl_FragColor = ...` assignment inside that chunk, so replacing the
// unexpanded `gl_FragColor` line (as older code did) silently no-ops. This
// reproduces the chunk's OPAQUE alpha handling, then multiplies outgoing light
// by the instance RGB and sets alpha from the instance A.
const FRAGMENT_OUTPUT = `#ifdef OPAQUE
  diffuseColor.a = 1.0;
#endif
gl_FragColor = vec4( outgoingLight * vInstanceColor.rgb, diffuseColor.a * vInstanceColor.a );`

/**
 * Patch a MeshBasicMaterial so it reads a per-instance vec4 RGBA from a
 * geometry attribute named `instanceColorRGBA` (Uint8Array, normalized).
 */
export function patchInstanceColorRGBA(material: THREE.MeshBasicMaterial): void {
  material.onBeforeCompile = (shader) => {
    shader.vertexShader = shader.vertexShader
      .replace('#include <common>', VERTEX_COMMON)
      .replace('#include <begin_vertex>', VERTEX_BEGIN)
    shader.fragmentShader = shader.fragmentShader
      .replace('#include <common>', FRAGMENT_COMMON)
      .replace('#include <opaque_fragment>', FRAGMENT_OUTPUT)
  }
}
