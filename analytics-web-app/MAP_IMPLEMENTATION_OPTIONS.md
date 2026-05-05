# Map Implementation Options

This document outlines different approaches to improve the map visualization feature. Review these options and decide which path to pursue based on your priorities and requirements.

---

## Current Status

**What Works:**
- ✅ React/Vite integration
- ✅ 3D map model loading (GLB/GLTF)
- ✅ Basic heatmap rendering
- ✅ Interactive markers with selection
- ✅ SQL query integration

**What Needs Improvement:**
- ❌ Performance with large datasets (currently limited to ~10K events)
- ❌ Heatmap efficiency (CPU-bound canvas rendering)
- ❌ No clustering for dense areas
- ❌ No spatial indexing/culling

---

## Option 1: Optimize Current React Three Fiber Implementation

Keep the existing R3F + Three.js stack and add performance optimizations.

### Sub-Option 1A: Instanced Rendering for Markers

**What it does:**
- Replaces individual mesh objects with a single instanced mesh
- Renders all markers in one draw call instead of thousands
- Massively improves GPU performance

**Implementation:**
```tsx
// Replace DeathMarkers component in MapViewer.tsx
import { useMemo, useRef } from 'react'
import * as THREE from 'three'

function InstancedDeathMarkers({ events, selectedId, onSelect }) {
  const meshRef = useRef<THREE.InstancedMesh>(null)
  const tempObject = useMemo(() => new THREE.Object3D(), [])

  useEffect(() => {
    if (!meshRef.current) return

    events.forEach((event, i) => {
      tempObject.position.set(event.x, event.z + 50, event.y)
      tempObject.scale.setScalar(event.id === selectedId ? 1.5 : 1)
      tempObject.updateMatrix()
      meshRef.current.setMatrixAt(i, tempObject.matrix)
    })

    meshRef.current.instanceMatrix.needsUpdate = true
  }, [events, selectedId])

  return (
    <instancedMesh ref={meshRef} args={[null, null, events.length]}>
      <sphereGeometry args={[30, 16, 16]} />
      <meshStandardMaterial color="#bf360c" />
    </instancedMesh>
  )
}
```

**Pros:**
- ✅ Can handle 100K+ markers efficiently
- ✅ Minimal code changes
- ✅ Stays within R3F ecosystem

**Cons:**
- ⚠️ Lose per-marker hover/click interactions (need raycasting)
- ⚠️ All markers share same geometry/material

**Effort:** Low (2-4 hours)
**Performance Gain:** High (10x-100x for large datasets)

---

### Sub-Option 1B: GPU-Based Heatmap with Shaders

**What it does:**
- Replaces CPU canvas rendering with WebGL fragment shader
- Calculates heatmap directly on GPU
- Real-time updates without recalculation

**Implementation:**
```tsx
// New component: GPUHeatmap.tsx
const heatmapShader = {
  vertexShader: `
    varying vec2 vUv;
    void main() {
      vUv = uv;
      gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    }
  `,
  fragmentShader: `
    uniform vec3 points[MAX_POINTS];
    uniform int pointCount;
    uniform float radius;
    uniform float intensity;
    varying vec2 vUv;

    void main() {
      float heat = 0.0;
      for (int i = 0; i < pointCount; i++) {
        vec2 pos = points[i].xy;
        float dist = distance(vUv, pos);
        if (dist < radius) {
          heat += intensity * (1.0 - dist / radius);
        }
      }

      // Heat color gradient
      vec3 color = mix(
        vec3(0.0, 0.0, 0.0),
        vec3(0.75, 0.21, 0.05), // #bf360c
        clamp(heat, 0.0, 1.0)
      );

      gl_FragColor = vec4(color, clamp(heat, 0.0, 0.7));
    }
  `
}

function GPUHeatmap({ events, radius, intensity }) {
  const shaderRef = useRef()

  useEffect(() => {
    if (!shaderRef.current) return

    // Update uniforms
    shaderRef.current.uniforms.pointCount.value = events.length
    shaderRef.current.uniforms.radius.value = radius
    shaderRef.current.uniforms.intensity.value = intensity

    // Update point positions
    events.forEach((event, i) => {
      shaderRef.current.uniforms.points.value[i].set(event.x, event.y, 0)
    })
  }, [events, radius, intensity])

  return (
    <mesh position={[0, 10, 0]} rotation={[-Math.PI / 2, 0, 0]}>
      <planeGeometry args={[10000, 10000]} />
      <shaderMaterial
        ref={shaderRef}
        vertexShader={heatmapShader.vertexShader}
        fragmentShader={heatmapShader.fragmentShader}
        uniforms={{
          points: { value: new Array(MAX_POINTS) },
          pointCount: { value: 0 },
          radius: { value: 50 },
          intensity: { value: 0.5 }
        }}
        transparent
        depthWrite={false}
      />
    </mesh>
  )
}
```

**Pros:**
- ✅ Real-time updates (no recalculation)
- ✅ Resolution-independent
- ✅ GPU-accelerated

**Cons:**
- ⚠️ Requires WebGL shader knowledge
- ⚠️ Limited by MAX_POINTS uniform array size (typically 1024-4096)
- ⚠️ More complex code

**Effort:** Medium (4-8 hours)
**Performance Gain:** High (real-time updates, no CPU bottleneck)

---

### Sub-Option 1C: Marker Clustering

**What it does:**
- Groups nearby markers when zoomed out
- Shows individual markers when zoomed in
- Reduces visual clutter and improves performance

**Implementation:**
```tsx
// Use a clustering library
import Supercluster from 'supercluster'

function useMarkerClusters(events: DeathEvent[], zoom: number) {
  const clusterIndex = useMemo(() => {
    const index = new Supercluster({
      radius: 40,
      maxZoom: 16
    })

    const points = events.map(event => ({
      type: 'Feature',
      properties: { ...event },
      geometry: {
        type: 'Point',
        coordinates: [event.x, event.y]
      }
    }))

    index.load(points)
    return index
  }, [events])

  const clusters = useMemo(() => {
    return clusterIndex.getClusters([-180, -85, 180, 85], Math.floor(zoom))
  }, [clusterIndex, zoom])

  return clusters
}

// Render clusters
function ClusteredMarkers({ events, zoom }) {
  const clusters = useMarkerClusters(events, zoom)

  return (
    <group>
      {clusters.map(cluster => {
        const [x, y] = cluster.geometry.coordinates
        const isCluster = cluster.properties.cluster

        if (isCluster) {
          const count = cluster.properties.point_count
          return (
            <mesh key={cluster.id} position={[x, 50, y]}>
              <sphereGeometry args={[30 * (1 + Math.log(count)), 16, 16]} />
              <meshStandardMaterial color="#bf360c" />
              <Html center>
                <div className="cluster-label">{count}</div>
              </Html>
            </mesh>
          )
        } else {
          return (
            <mesh key={cluster.properties.id} position={[x, 50, y]}>
              <sphereGeometry args={[30, 16, 16]} />
              <meshStandardMaterial color="#bf360c" />
            </mesh>
          )
        }
      })}
    </group>
  )
}
```

**Pros:**
- ✅ Handles millions of points
- ✅ Better UX (less visual clutter)
- ✅ Automatic zoom-based detail level

**Cons:**
- ⚠️ Additional dependency (supercluster ~50KB)
- ⚠️ Need to track zoom level
- ⚠️ Cluster expansion interaction needed

**Effort:** Medium (6-10 hours)
**Performance Gain:** High (reduces rendered objects by 10x-100x)

---

### Sub-Option 1D: Spatial Indexing with Frustum Culling

**What it does:**
- Only renders markers visible in camera view
- Uses octree or R-tree for spatial queries
- Reduces draw calls for off-screen objects

**Implementation:**
```tsx
import { Octree } from 'three/examples/jsm/math/Octree'

function useFrustumCulling(events: DeathEvent[], camera: THREE.Camera) {
  const octree = useMemo(() => {
    const tree = new Octree()
    events.forEach(event => {
      const sphere = new THREE.Sphere(
        new THREE.Vector3(event.x, event.z, event.y),
        30
      )
      tree.add({ sphere, event })
    })
    return tree
  }, [events])

  const visibleEvents = useMemo(() => {
    const frustum = new THREE.Frustum()
    const projScreenMatrix = new THREE.Matrix4()
    projScreenMatrix.multiplyMatrices(
      camera.projectionMatrix,
      camera.matrixWorldInverse
    )
    frustum.setFromProjectionMatrix(projScreenMatrix)

    return octree.search(frustum)
  }, [octree, camera])

  return visibleEvents
}
```

**Pros:**
- ✅ Only renders visible objects
- ✅ Scales to very large datasets
- ✅ Automatic performance optimization

**Cons:**
- ⚠️ Complex implementation
- ⚠️ Need to rebuild octree on data changes
- ⚠️ Requires camera update tracking

**Effort:** High (10-16 hours)
**Performance Gain:** Medium-High (depends on viewport coverage)

---

### Option 1 Summary: Full Optimization Package

**Combine all sub-options for maximum performance:**

1. Instanced rendering for markers (1A)
2. GPU heatmap shader (1B)
3. Marker clustering (1C)
4. Spatial indexing (1D)

**Total Effort:** 3-5 days
**Expected Result:** Handle 1M+ events smoothly
**Recommendation:** Implement incrementally (1A → 1C → 1B → 1D)

---

## Option 2: Hybrid Approach (R3F + Deck.gl)

Use React Three Fiber for 3D map models and Deck.gl for data visualization layers.

### Architecture

```tsx
import DeckGL from '@deck.gl/react'
import { HeatmapLayer, ScatterplotLayer } from '@deck.gl/layers'
import { Canvas } from '@react-three/fiber'

function HybridMapViewer({ mapUrl, deathEvents }) {
  return (
    <div style={{ position: 'relative', width: '100%', height: '100%' }}>
      {/* Three.js layer for 3D map */}
      <Canvas style={{ position: 'absolute', top: 0, left: 0 }}>
        <MapModel url={mapUrl} />
        <OrthographicCamera />
        <MapControls />
      </Canvas>

      {/* Deck.gl layer for data visualization */}
      <DeckGL
        style={{ position: 'absolute', top: 0, left: 0, pointerEvents: 'none' }}
        layers={[
          new HeatmapLayer({
            id: 'death-heatmap',
            data: deathEvents,
            getPosition: d => [d.x, d.y],
            getWeight: 1,
            radiusPixels: 50,
            intensity: 0.5
          }),
          new ScatterplotLayer({
            id: 'death-markers',
            data: deathEvents,
            getPosition: d => [d.x, d.y],
            getRadius: 30,
            getFillColor: [191, 54, 12],
            pickable: true,
            onClick: info => console.log(info.object)
          })
        ]}
      />
    </div>
  )
}
```

### Implementation Steps

1. **Install Deck.gl**
   ```bash
   yarn add @deck.gl/core @deck.gl/layers @deck.gl/react
   ```

2. **Synchronize Cameras**
   - Both R3F and Deck.gl need same view matrix
   - Share camera state between canvases

3. **Layer Ordering**
   - R3F canvas renders 3D map
   - Deck.gl canvas renders data on top
   - Use CSS z-index for layering

### Pros

- ✅ Best of both worlds (3D models + optimized data viz)
- ✅ Deck.gl handles large datasets out-of-the-box
- ✅ Built-in GPU heatmaps and clustering
- ✅ 50+ layer types available
- ✅ No manual optimization needed

### Cons

- ❌ Complex camera synchronization
- ❌ Two rendering contexts (more memory)
- ❌ Additional 800KB bundle size (Deck.gl)
- ❌ Coordinate system mapping between layers
- ❌ Pointer event handling complexity
- ❌ More dependencies to maintain

### Effort

**High (1-2 weeks)**
- Camera synchronization: 2-3 days
- Layer integration: 2-3 days
- Event handling: 1-2 days
- Testing and debugging: 2-3 days

### Performance Gain

**Very High** - Immediate large dataset support without manual optimization

### When to Choose This Option

- ✅ Need large dataset performance NOW
- ✅ Want built-in data visualization features
- ✅ Have complex data visualization requirements
- ✅ Budget for increased complexity

---

## Option 3: Switch to Deck.gl Only (No 3D Maps)

Replace R3F entirely with Deck.gl for 2D/2.5D visualization.

### What You Lose

- ❌ Cannot load 3D game maps (GLB/GLTF)
- ❌ No true 3D scene rendering
- ❌ Limited to layer-based visualization

### What You Gain

- ✅ Excellent large dataset performance
- ✅ Built-in heatmaps, clustering, aggregation
- ✅ Simpler architecture (one renderer)
- ✅ Better documentation for data viz

### Implementation

```tsx
import DeckGL from '@deck.gl/react'
import { HeatmapLayer, ScatterplotLayer, BitmapLayer } from '@deck.gl/layers'

function DeckGLMapViewer({ deathEvents, mapImageUrl }) {
  return (
    <DeckGL
      initialViewState={{
        longitude: 0,
        latitude: 0,
        zoom: 10
      }}
      controller={true}
      layers={[
        // Background map image
        new BitmapLayer({
          id: 'map-background',
          image: mapImageUrl,
          bounds: [minX, minY, maxX, maxY]
        }),
        // Heatmap
        new HeatmapLayer({
          id: 'death-heatmap',
          data: deathEvents,
          getPosition: d => [d.x, d.y],
          radiusPixels: 50
        }),
        // Markers
        new ScatterplotLayer({
          id: 'death-markers',
          data: deathEvents,
          getPosition: d => [d.x, d.y],
          getRadius: 30,
          getFillColor: [191, 54, 12]
        })
      ]}
    />
  )
}
```

### Pros

- ✅ Maximum performance for data visualization
- ✅ Simpler architecture
- ✅ Built-in features (clustering, brushing, filtering)
- ✅ Excellent for 2D tactical maps

### Cons

- ❌ No 3D model support
- ❌ Need pre-rendered map images
- ❌ Less flexible for custom 3D visualizations

### Effort

**Medium (1 week)**
- Replace R3F components: 2-3 days
- Update data pipeline: 1-2 days
- Styling and interactions: 1-2 days

### When to Choose This Option

- ✅ Don't need 3D game maps
- ✅ Have 2D map images available
- ✅ Large dataset performance is critical
- ❌ Not recommended if 3D maps are required

---

## Option 4: Switch to Babylon.js

Replace R3F with Babylon.js for more game engine features.

### What Changes

- Complete rewrite of MapViewer component
- Imperative API instead of declarative React
- More boilerplate code

### What You Gain

- ✅ Built-in scene inspector (debugging)
- ✅ Physics engine
- ✅ Advanced animation system
- ✅ Better built-in optimization tools (LOD, culling)

### Implementation Complexity

```tsx
import { Engine, Scene, ArcRotateCamera, HemisphericLight } from '@babylonjs/core'
import '@babylonjs/loaders' // GLB/GLTF support

function BabylonMapViewer({ mapUrl, deathEvents }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    if (!canvasRef.current) return

    // Imperative setup
    const engine = new Engine(canvasRef.current)
    const scene = new Scene(engine)
    const camera = new ArcRotateCamera('camera', 0, 0, 100, Vector3.Zero(), scene)
    const light = new HemisphericLight('light', new Vector3(0, 1, 0), scene)

    // Load map
    SceneLoader.ImportMesh('', '', mapUrl, scene)

    // Create markers
    const sphere = MeshBuilder.CreateSphere('marker', { diameter: 30 }, scene)
    deathEvents.forEach(event => {
      const instance = sphere.createInstance(`marker-${event.id}`)
      instance.position = new Vector3(event.x, event.z, event.y)
    })

    engine.runRenderLoop(() => scene.render())

    return () => {
      scene.dispose()
      engine.dispose()
    }
  }, [mapUrl, deathEvents])

  return <canvas ref={canvasRef} style={{ width: '100%', height: '100%' }} />
}
```

### Pros

- ✅ More built-in game engine features
- ✅ Better debugging tools (inspector)
- ✅ Built-in optimization features
- ✅ VR/XR support

### Cons

- ❌ Complete rewrite needed
- ❌ Less React-friendly (imperative API)
- ❌ Larger bundle size (~1.5MB)
- ❌ Harder to integrate with React state

### Effort

**Very High (2-3 weeks)**
- Rewrite all 3D components: 1 week
- Re-implement interactions: 3-5 days
- Testing and debugging: 3-5 days

### When to Choose This Option

- ✅ Need game engine features (physics, advanced animations)
- ✅ VR/AR is a future requirement
- ✅ Want built-in debugging tools
- ❌ Not recommended for this project (too much rewrite)

---

## Option 5: Minimal Approach - Keep Current, Add Quick Wins Only

Make minimal changes to address most critical issues.

### Quick Wins (1-2 days total)

1. **Add Instanced Rendering** (Sub-option 1A)
   - 2-4 hours
   - 10x performance improvement

2. **Add "Fit to Data" Camera Button**
   ```tsx
   function fitCameraToData(camera, controls, events) {
     const box = new THREE.Box3()
     events.forEach(event => {
       box.expandByPoint(new THREE.Vector3(event.x, event.z, event.y))
     })
     const center = box.getCenter(new THREE.Vector3())
     const size = box.getSize(new THREE.Vector3())
     const maxDim = Math.max(size.x, size.y, size.z)
     camera.position.set(center.x, maxDim * 2, center.z)
     controls.target.copy(center)
   }
   ```
   - 1-2 hours

3. **Add Event Count Warning**
   ```tsx
   {deathEvents.length > 10000 && (
     <div className="warning">
       Showing {deathEvents.length.toLocaleString()} events.
       Performance may be impacted. Consider narrowing time range.
     </div>
   )}
   ```
   - 30 minutes

### Total Effort: 4-7 hours
### Result: 10x performance, better UX, minimal risk

---

## Decision Matrix

| Option | Effort | Performance | 3D Maps | Features | Risk | Recommended |
|--------|--------|-------------|---------|----------|------|-------------|
| **1. Optimize R3F (Full)** | High (3-5 days) | Excellent | ✅ | Custom | Low | ⭐⭐⭐⭐⭐ |
| **1A. Instanced Only** | Low (4h) | Good | ✅ | Current | Very Low | ⭐⭐⭐⭐ |
| **1C. + Clustering** | Medium (10h) | Very Good | ✅ | + Clustering | Low | ⭐⭐⭐⭐⭐ |
| **2. Hybrid R3F+Deck.gl** | Very High (2w) | Excellent | ✅ | Many | Medium | ⭐⭐⭐ |
| **3. Deck.gl Only** | Medium (1w) | Excellent | ❌ | Many | Medium | ⭐⭐ |
| **4. Babylon.js** | Very High (3w) | Excellent | ✅ | Game Engine | High | ⭐ |
| **5. Minimal Quick Wins** | Very Low (7h) | Good | ✅ | Current | Very Low | ⭐⭐⭐⭐ |

---

## Recommendation by Scenario

### If Performance with Large Datasets is CRITICAL NOW
**→ Option 2: Hybrid R3F + Deck.gl**
- Immediate large dataset support
- Keep 3D map capability
- Accept increased complexity

### If You Want Best Long-Term Solution
**→ Option 1: Optimize R3F (Full Package)**
- Implement incrementally: 1A → 1C → 1B → 1D
- Maximum flexibility and performance
- Stays within current stack

### If You Want Quick Improvement with Minimal Risk
**→ Option 5: Minimal Quick Wins**
- Start with instanced rendering (1A)
- Add camera controls
- Evaluate if more optimization is needed

### If 3D Maps Are NOT Required
**→ Option 3: Deck.gl Only**
- Simplest high-performance solution
- But requires giving up 3D map models

---

## Next Steps

1. **Review this document** and identify your priorities:
   - [ ] Performance with large datasets (how many events?)
   - [ ] 3D map model support (required or optional?)
   - [ ] Development timeline (urgent or gradual?)
   - [ ] Team capacity (available hours/days?)

2. **Choose an option** based on priorities

3. **Start implementation** or request detailed implementation plan

4. **Test with real data** at target scale

---

## Questions to Help Decide

1. **How many death events do you expect to visualize?**
   - < 10K → Current implementation works
   - 10K-100K → Instanced rendering (Option 1A) sufficient
   - 100K-1M → Need clustering (Option 1C) or Deck.gl
   - > 1M → Hybrid approach (Option 2) or Deck.gl only

2. **Are 3D game maps (GLB/GLTF) required?**
   - Yes → Must keep R3F (Options 1, 2, or 4)
   - No → Can use Deck.gl only (Option 3)

3. **What's your timeline?**
   - < 1 day → Option 5 (minimal quick wins)
   - 1 week → Option 1A or 1C
   - 2+ weeks → Option 1 (full) or Option 2

4. **Do you have WebGL/shader expertise?**
   - Yes → Can do Option 1B (GPU heatmap)
   - No → Skip shaders, use instancing and clustering

5. **Are there other data visualization needs?**
   - Just heatmaps → Current approach sufficient
   - Complex multi-layer viz → Consider Deck.gl

---

## Contact Points for Implementation

When ready to implement, request:
- [ ] Detailed implementation guide for chosen option
- [ ] Code review of current implementation
- [ ] Performance benchmarking script
- [ ] Testing plan with sample data
