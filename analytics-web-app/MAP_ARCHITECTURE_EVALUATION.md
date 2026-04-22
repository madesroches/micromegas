# Map Visualization Architecture Evaluation

## Current Implementation: React Three Fiber + Three.js

### Overview

The current implementation uses React Three Fiber (R3F) as a React renderer for Three.js, along with the Drei helper library. This stack provides a declarative way to build 3D scenes within the React ecosystem.

**Technology Stack:**
- **Three.js** (v0.182.0) - Core 3D graphics library
- **@react-three/fiber** (v9.5.0) - React renderer for Three.js
- **@react-three/drei** (v10.7.7) - Helper components and utilities

### Architecture Components

```
MapPage (Route)
├── QueryEditor (SQL editing + time range)
├── UI Controls
│   ├── Markers toggle
│   ├── Heatmap toggle + radius/intensity sliders
│   ├── Fit to Data button
│   └── Mock Data toggle + count selector
├── MapViewer (3D Canvas)
│   ├── OrthographicCamera (top-down view)
│   ├── CameraController (initial position)
│   ├── FitToDataController (auto-fit camera to data bounds)
│   ├── MapControls (pan/zoom)
│   ├── MapModel (GLB/GLTF loader)
│   ├── HeatmapLayer (canvas texture)
│   ├── InstancedDeathMarkers (instanced rendering)
│   └── PlaceholderGrid (fallback)
├── DeathDetailPanel (event details overlay)
└── Mock Data Generator (for testing without backend)

useMapData (Hook)
├── SQL query execution
├── Arrow table processing
└── DeathEvent transformation
```

### Pros

#### Integration & Developer Experience
- ✅ **React Native Integration**: Declarative JSX syntax matches existing React patterns
- ✅ **Component Composition**: Natural React component hierarchy for 3D scenes
- ✅ **Hooks Compatibility**: Works seamlessly with React hooks (useState, useEffect, etc.)
- ✅ **TypeScript Support**: Excellent type definitions for both R3F and Three.js
- ✅ **Existing Ecosystem**: Leverages existing Vite + React toolchain

#### Performance & Capabilities
- ✅ **Three.js Foundation**: Access to full Three.js ecosystem and features
- ✅ **Automatic Optimization**: R3F handles render loop and updates efficiently
- ✅ **Model Loading**: Built-in support for GLB/GLTF via Drei
- ✅ **Drei Helpers**: Camera controls, loaders, and utilities out-of-the-box
- ✅ **WebGL Performance**: Hardware-accelerated 3D rendering
- ✅ **Instanced Rendering**: Implemented for markers (100K+ points)

#### Flexibility
- ✅ **Extensible**: Easy to add custom shaders, geometries, materials
- ✅ **3D Primitives**: Full access to meshes, lights, cameras, etc.
- ✅ **Custom Rendering**: Can implement any visualization technique
- ✅ **Active Community**: Large community, good documentation

### Cons

#### Complexity & Learning Curve
- ❌ **Three.js Knowledge Required**: Developers need to understand 3D graphics concepts
- ❌ **R3F Abstraction**: Additional layer to learn on top of Three.js
- ❌ **Manual Optimization**: Need to manually handle instancing, LOD, etc.
- ❌ **Coordinate System Complexity**: Manual mapping between game coordinates and 3D space

#### Performance Concerns
- ✅ **Marker Scalability**: Solved with instanced rendering (100K+ markers)
- ❌ **No Built-in Clustering**: Need to implement marker clustering manually
- ❌ **Heatmap Inefficiency**: Canvas-based heatmap recalculates entire texture on changes
- ❌ **Memory Management**: Need to manually dispose geometries and textures

#### Features
- ❌ **No Map Tiles**: Not designed for geographic/tile-based maps
- ❌ **No Built-in Data Viz**: Need to build heatmaps, clustering, etc. from scratch
- ❌ **Limited 2D Overlays**: Mixing HTML overlays with 3D requires careful positioning
- ❌ **No GIS Support**: No built-in geographic coordinate systems

#### Development Experience
- ❌ **Debugging Difficulty**: 3D scene debugging is harder than 2D
- ❌ **Boilerplate Code**: Requires setup of cameras, lights, controls
- ❌ **Performance Profiling**: Need external tools to profile WebGL performance

### Current Implementation Status

#### ✅ Implemented

1. **Instanced Rendering** (Performance)
   - Uses `THREE.InstancedMesh` for all markers (single draw call)
   - Per-instance colors via `InstancedBufferAttribute`
   - Handles 100K+ markers efficiently
   - Preserves click/hover interactions via `instanceId`

2. **Fit to Data Camera Control**
   - "Fit to Data" button centers camera on all data points
   - Calculates bounding box and adjusts zoom automatically
   - Works with orthographic camera

3. **Mock Data Generator** (Testing)
   - Toggle to use mock data without backend
   - Configurable event count (100 to 100K)
   - Clustered distribution around hotspots
   - Random player names and death causes

#### ⚠️ Partially Implemented

1. **Performance at Scale**
   - ✅ Instanced mesh rendering for markers
   - ❌ Heatmap still uses CPU canvas rendering
   - ❌ No LOD (Level of Detail) system

2. **Camera System**
   - ✅ "Fit to bounds" functionality added
   - ❌ Rotation locked (orthographic top-down only)
   - ❌ No camera position persistence

#### ❌ Not Yet Implemented

1. **Data Visualization**
   - Fixed heatmap resolution (1024×1024)
   - No temporal animation/playback
   - No marker clustering for dense areas

2. **Map Loading**
   - No error handling for failed GLB loads
   - Object URL cleanup missing
   - No map metadata or preview

3. **Advanced Features**
   - GPU-based heatmap shader
   - Spatial indexing/frustum culling
   - Web Workers for data processing

### Best Suited For

- ✅ Custom 3D game map visualizations
- ✅ Complex 3D interactions
- ✅ Custom shader effects
- ✅ 3D model viewing/manipulation
- ✅ Projects already using React + Three.js

---

## Alternative Architecture Options

### Option 1: Deck.gl

**Website:** https://deck.gl
**Maintainer:** Uber, OpenJS Foundation

#### Overview

Deck.gl is a WebGL-powered framework for visual exploratory data analysis of large datasets. It's built for high-performance rendering of large-scale data visualizations with built-in layers for common patterns.

**Core Stack:**
```javascript
import DeckGL from '@deck.gl/react';
import { HeatmapLayer, ScatterplotLayer } from '@deck.gl/layers';
```

#### Pros

##### Performance
- ✅ **GPU-Accelerated Layers**: Optimized layers for millions of data points
- ✅ **Automatic Instancing**: Built-in instanced rendering
- ✅ **Efficient Updates**: Smart diffing for layer updates
- ✅ **WebGL2 Support**: Leverages modern WebGL features

##### Data Visualization
- ✅ **Built-in Heatmaps**: High-performance GPU-based heatmaps
- ✅ **50+ Layer Types**: Scatterplot, hexagon, grid, arc, line, etc.
- ✅ **Aggregation Layers**: CPU/GPU aggregation for dense data
- ✅ **Brushing/Filtering**: Built-in interactive data filtering

##### Geographic Features
- ✅ **GIS Integration**: Works with Mapbox, Google Maps, OpenStreetMap
- ✅ **Coordinate Systems**: Built-in support for lat/lng, Web Mercator, etc.
- ✅ **Tile Layers**: Can render base map tiles
- ✅ **Viewport Management**: Sophisticated camera controls

##### Developer Experience
- ✅ **React Integration**: Official React bindings
- ✅ **TypeScript**: Full TypeScript support
- ✅ **Documentation**: Excellent docs and examples
- ✅ **Active Development**: Regular updates from Uber/Vis.gl team

#### Cons

##### Flexibility
- ❌ **Layer-Based**: Less flexible than raw Three.js for custom 3D
- ❌ **Limited 3D Models**: Not designed for loading game maps (GLB/GLTF)
- ❌ **Constrained to Layers**: Custom visualizations require writing custom layers
- ❌ **Geographic Focus**: Optimized for geo data, not arbitrary 3D spaces

##### Integration
- ❌ **Base Map Dependency**: Works best with a map provider (Mapbox, etc.)
- ❌ **Coordinate System**: Requires mapping game coordinates to geographic system
- ❌ **Bundle Size**: Larger bundle (~800KB minified)

##### Use Cases
- ❌ **Not for Game Maps**: Not designed for 3D game environment visualization
- ❌ **Limited Custom 3D**: Hard to add custom 3D models/scenes

#### Best Suited For

- ✅ Large-scale point cloud visualization
- ✅ Geographic heatmaps and data overlays
- ✅ Time-series animation of spatial data
- ✅ Interactive data exploration dashboards
- ⚠️ **Not ideal for custom game map visualization**

---

### Option 2: Cesium / Cesium for Unreal

**Website:** https://cesium.com
**License:** Apache 2.0

#### Overview

Cesium is a platform for 3D geospatial visualization, designed for globe-scale and high-precision 3D mapping. It supports photogrammetry, 3D tiles, terrain, and time-dynamic data.

**Core Stack:**
```javascript
import { Viewer, Ion } from 'cesium';
```

#### Pros

##### 3D Geospatial
- ✅ **Globe Rendering**: Full 3D globe with terrain
- ✅ **3D Tiles**: Streaming 3D content (buildings, point clouds)
- ✅ **High Precision**: Sub-meter accuracy for coordinates
- ✅ **Time-Dynamic**: Built-in timeline for temporal data

##### Visualization
- ✅ **Point Cloud Support**: Native point cloud rendering
- ✅ **Photogrammetry**: Can load photogrammetry models
- ✅ **GLTF Support**: Native support for 3D models
- ✅ **Particle Systems**: Built-in particle effects

##### Integration
- ✅ **Unreal Engine**: Has Cesium for Unreal plugin (relevant for game integration)
- ✅ **Standards-Based**: Supports OGC standards (3D Tiles, WMS, WFS)

#### Cons

##### Complexity
- ❌ **Steep Learning Curve**: Complex API for geospatial concepts
- ❌ **Geographic Focus**: Designed for Earth-based coordinates
- ❌ **Overkill for Simple Maps**: Heavy-weight for 2D/simple 3D visualization

##### Performance
- ❌ **Large Bundle**: ~10MB+ uncompressed
- ❌ **Resource Intensive**: High memory usage for terrain/tiles
- ❌ **Startup Time**: Slower initial load

##### Integration
- ❌ **No Official React Bindings**: Community wrappers only (resium)
- ❌ **Coordinate System Mismatch**: Game coordinates don't map to globe easily
- ❌ **Cesium Ion**: Many features require Cesium Ion account

##### Cost
- ⚠️ **Pricing**: Some features require paid Cesium Ion subscription

#### Best Suited For

- ✅ Globe-scale geospatial visualization
- ✅ Terrain and elevation data
- ✅ Integration with Unreal Engine projects
- ⚠️ **Overkill for game map heatmaps**

---

### Option 3: Mapbox GL JS

**Website:** https://www.mapbox.com/mapbox-gljs
**License:** Proprietary (free tier available)

#### Overview

Mapbox GL JS is a JavaScript library for interactive, customizable vector maps rendered with WebGL. Strong focus on 2D maps with some 3D capabilities.

**Core Stack:**
```javascript
import mapboxgl from 'mapbox-gl';
import 'mapbox-gl/dist/mapbox-gl.css';
```

#### Pros

##### Map Features
- ✅ **Vector Tiles**: Smooth, scalable map rendering
- ✅ **Custom Styling**: Full control over map appearance
- ✅ **3D Terrain**: Hillshading and 3D terrain support
- ✅ **3D Buildings**: Can extrude buildings in 3D

##### Performance
- ✅ **WebGL Optimized**: Highly optimized rendering engine
- ✅ **Tile Caching**: Efficient tile management
- ✅ **Smooth Animations**: Built-in smooth transitions

##### Data Visualization
- ✅ **Heatmap Layer**: Built-in GPU-accelerated heatmaps
- ✅ **Data-Driven Styling**: Style based on data properties
- ✅ **Clustering**: Built-in marker clustering

##### Developer Experience
- ✅ **Excellent Docs**: Comprehensive documentation
- ✅ **React Wrapper**: react-map-gl from Uber
- ✅ **Ecosystem**: Large ecosystem of plugins

#### Cons

##### Geographic Focus
- ❌ **Requires Base Map**: Needs map tiles (costs or self-hosting)
- ❌ **Geographic Coordinates**: Designed for lat/lng, not game coordinates
- ❌ **Limited 3D**: 3D features are limited compared to full 3D engines

##### Licensing & Cost
- ❌ **Proprietary**: Not open source
- ❌ **API Token Required**: Requires Mapbox account
- ❌ **Pricing**: Costs money beyond free tier (50K monthly active users)
- ❌ **Vendor Lock-in**: Tied to Mapbox ecosystem

##### Use Case Mismatch
- ❌ **Not for Game Maps**: Not designed for game environment visualization
- ❌ **No Custom 3D Models**: Can't load GLB/GLTF game maps
- ❌ **2D Focused**: Primarily a 2D mapping library

#### Best Suited For

- ✅ Geographic web maps
- ✅ Location-based analytics
- ✅ Real-time tracking dashboards
- ⚠️ **Not suitable for game map visualization**

---

### Option 4: Babylon.js

**Website:** https://www.babylonjs.com
**License:** Apache 2.0

#### Overview

Babylon.js is a powerful, open-source 3D engine built with TypeScript. It's a complete game engine for the web with a focus on developer experience and performance.

**Core Stack:**
```javascript
import { Engine, Scene, ArcRotateCamera } from '@babylonjs/core';
```

#### Pros

##### Features
- ✅ **Complete Game Engine**: Physics, particles, audio, AI, etc.
- ✅ **Inspector Tool**: Built-in scene inspector for debugging
- ✅ **Material Editor**: Visual material/shader editor (Node Material Editor)
- ✅ **Animation System**: Advanced animation and skeletal animation
- ✅ **VR/XR Support**: Built-in WebXR support

##### Performance
- ✅ **Highly Optimized**: Excellent performance optimizations
- ✅ **LOD System**: Built-in Level of Detail
- ✅ **Instancing**: Easy mesh instancing
- ✅ **Occlusion Culling**: Built-in culling systems

##### Developer Experience
- ✅ **TypeScript First**: Written in TypeScript with excellent types
- ✅ **Playground**: Online playground for testing
- ✅ **Documentation**: Comprehensive docs and tutorials
- ✅ **Active Community**: Strong community support

##### 3D Models
- ✅ **GLTF/GLB Support**: Excellent model loading
- ✅ **Multiple Formats**: Supports many 3D formats
- ✅ **Material Systems**: PBR materials, custom shaders

#### Cons

##### React Integration
- ❌ **No Official React Renderer**: No equivalent to React Three Fiber
- ❌ **Imperative API**: Doesn't fit React's declarative paradigm
- ❌ **Manual DOM Management**: Need to manage canvas lifecycle

##### Bundle Size
- ❌ **Large Core**: Larger than Three.js (~1.5MB minified)
- ❌ **Tree-Shaking**: Harder to tree-shake than Three.js

##### Ecosystem
- ❌ **Smaller Ecosystem**: Less third-party libraries than Three.js
- ❌ **Learning Curve**: Game engine concepts (scenes, managers, etc.)

##### Integration
- ❌ **More Setup Required**: More boilerplate than R3F
- ❌ **State Management**: Harder to sync with React state

#### Best Suited For

- ✅ Complex 3D web applications
- ✅ Web-based games
- ✅ VR/AR experiences
- ✅ Advanced 3D visualizations
- ⚠️ **Good option but less React-friendly than R3F**

---

### Option 5: Pure Three.js (without React Three Fiber)

**Website:** https://threejs.org
**License:** MIT

#### Overview

Use Three.js directly without the React Three Fiber abstraction layer. Manage the 3D scene imperatively in useEffect hooks or a dedicated manager.

**Core Stack:**
```javascript
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls';
```

#### Pros

##### Control & Performance
- ✅ **Direct Control**: Full control over rendering loop
- ✅ **No Abstraction Overhead**: Slightly better performance than R3F
- ✅ **Lighter Bundle**: No R3F dependency (~150KB savings)
- ✅ **Lower Learning Curve**: Only need to learn Three.js, not R3F

##### Flexibility
- ✅ **No Framework Constraints**: Not bound to React reconciliation
- ✅ **Custom Render Loop**: Full control over update/render timing
- ✅ **Direct WebGL**: Easier to drop down to raw WebGL if needed

#### Cons

##### Integration
- ❌ **Imperative in React**: Doesn't fit React's declarative model
- ❌ **Manual State Sync**: Hard to sync Three.js state with React state
- ❌ **Lifecycle Management**: Manual setup/teardown in useEffect
- ❌ **No JSX**: Can't use JSX for scene composition

##### Developer Experience
- ❌ **More Boilerplate**: Need to manually manage scene, camera, renderer
- ❌ **Ref-Heavy Code**: Lots of useRef for Three.js objects
- ❌ **Harder to Compose**: Can't compose scenes with React components

##### Maintenance
- ❌ **Manual Optimization**: No automatic reconciliation/updates
- ❌ **More Code**: Generally more verbose than R3F equivalent

#### Best Suited For

- ✅ Performance-critical 3D rendering
- ✅ Non-React projects
- ✅ Simple 3D viewers (not complex scenes)
- ⚠️ **Similar capabilities to R3F but less ergonomic in React**

---

### Option 6: Plotly.js

**Website:** https://plotly.com/javascript/
**License:** MIT (open source version)

#### Overview

Plotly is a high-level declarative charting library with support for 3D scatter plots, surface plots, and other scientific visualizations.

**Core Stack:**
```javascript
import Plotly from 'plotly.js-dist';
```

#### Pros

##### Ease of Use
- ✅ **Declarative API**: Simple JSON-based configuration
- ✅ **Fast Setup**: Minimal code for basic visualizations
- ✅ **React Wrapper**: Official react-plotly.js component

##### Built-in Features
- ✅ **Heatmaps**: Built-in 2D and 3D heatmaps
- ✅ **3D Scatter**: 3D scatter plots out of the box
- ✅ **Interactivity**: Hover, zoom, pan built-in
- ✅ **Styling**: Extensive styling options

#### Cons

##### Limitations
- ❌ **No 3D Models**: Can't load custom 3D maps (GLB/GLTF)
- ❌ **Limited Customization**: Hard to customize beyond provided options
- ❌ **Not for Game Maps**: Designed for scientific/statistical visualization
- ❌ **Large Bundle**: Full build is ~3MB

##### Performance
- ❌ **SVG/Canvas Based**: Not WebGL-optimized (except for some modes)
- ❌ **Limited Scalability**: Performance degrades with large datasets

#### Best Suited For

- ✅ Scientific data visualization
- ✅ Statistical charts and plots
- ✅ Quick prototyping
- ⚠️ **Not suitable for game map visualization**

---

### Option 7: PixiJS + Custom 2D Overlay

**Website:** https://pixijs.com
**License:** MIT

#### Overview

PixiJS is a 2D WebGL renderer. Could be used for a top-down 2D view with high-performance sprite rendering for markers.

**Core Stack:**
```javascript
import * as PIXI from 'pixi.js';
```

#### Pros

##### 2D Performance
- ✅ **Extremely Fast**: Optimized for 2D sprite rendering
- ✅ **WebGL Accelerated**: Hardware-accelerated 2D
- ✅ **Particle Systems**: Excellent particle effects
- ✅ **Small Bundle**: Lighter than 3D engines

##### Developer Experience
- ✅ **Simple API**: Easier than 3D engines
- ✅ **React Integration**: react-pixi available
- ✅ **Filters**: Built-in filters and effects

#### Cons

##### 2D Only
- ❌ **No 3D**: Strictly 2D rendering
- ❌ **No 3D Models**: Can't use 3D game maps
- ❌ **Perspective**: Top-down only, no camera angles

##### Use Case
- ❌ **Limited for This Project**: Loses 3D capabilities
- ❌ **Map Images Required**: Need pre-rendered map images

#### Best Suited For

- ✅ 2D top-down tactical views
- ✅ High-performance 2D dashboards
- ✅ Simple marker-based maps
- ⚠️ **Viable if dropping 3D requirement**

---

## Comparison Matrix

| Feature | R3F + Three.js | Deck.gl | Cesium | Mapbox GL | Babylon.js | Pure Three.js | Plotly | PixiJS |
|---------|---------------|---------|---------|-----------|-----------|---------------|--------|--------|
| **3D Game Maps (GLB/GLTF)** | ✅ Excellent | ❌ No | ✅ Yes | ❌ No | ✅ Excellent | ✅ Excellent | ❌ No | ❌ No |
| **React Integration** | ✅ Native | ✅ Good | ⚠️ Community | ✅ Good | ❌ Manual | ⚠️ Manual | ✅ Official | ⚠️ Community |
| **Performance (10K+ points)** | ✅ Good (instanced) | ✅ Excellent | ✅ Good | ✅ Good | ✅ Excellent | ⚠️ Manual | ❌ Limited | ✅ Good |
| **Heatmaps** | ⚠️ Manual | ✅ Built-in | ✅ Built-in | ✅ Built-in | ⚠️ Manual | ⚠️ Manual | ✅ Built-in | ⚠️ Manual |
| **Learning Curve** | Medium | Medium | High | Medium | Medium-High | Medium | Low | Low-Medium |
| **Bundle Size** | ~500KB | ~800KB | ~10MB | ~400KB | ~1.5MB | ~350KB | ~3MB | ~300KB |
| **License** | MIT | MIT | Apache 2.0 | Proprietary | Apache 2.0 | MIT | MIT | MIT |
| **Cost** | Free | Free | Free/Paid | Paid | Free | Free | Free | Free |
| **Geographic Features** | ❌ No | ✅ Excellent | ✅ Excellent | ✅ Excellent | ❌ No | ❌ No | ❌ No | ❌ No |
| **Custom 3D Scenes** | ✅ Full Control | ❌ Limited | ⚠️ Limited | ❌ No | ✅ Full Control | ✅ Full Control | ❌ No | ❌ No |
| **TypeScript Support** | ✅ Excellent | ✅ Excellent | ⚠️ Good | ✅ Good | ✅ Excellent | ✅ Excellent | ⚠️ Good | ✅ Good |
| **Documentation** | ✅ Good | ✅ Excellent | ✅ Excellent | ✅ Excellent | ✅ Excellent | ✅ Excellent | ✅ Good | ✅ Good |
| **Best For This Project** | ✅ Strong | ❌ No | ❌ Overkill | ❌ No | ✅ Alternative | ⚠️ Similar | ❌ No | ❌ No |

---

## Recommendation

### For Game Map Visualization with 3D Models

**🏆 Current Choice (React Three Fiber + Three.js) is the best fit**

**Reasoning:**
1. ✅ **3D Model Support**: Can load GLB/GLTF game maps
2. ✅ **React Integration**: Fits existing architecture
3. ✅ **Flexibility**: Full control over custom visualizations
4. ✅ **Cost**: Free and open source
5. ✅ **Community**: Large ecosystem and support

**Alternative Worth Considering:**
- **Babylon.js**: If you need more game engine features (physics, advanced animations, VR)
  - Trade-off: Less React-friendly, more complex setup

### Optimization Path for Current Implementation

Instead of switching technologies, these improvements are being implemented:

1. **✅ Instanced Rendering** (DONE)
   ```typescript
   // InstancedDeathMarkers component uses InstancedMesh
   <instancedMesh
     ref={meshRef}
     args={[geometry, material, events.length]}
     onClick={handleClick}
     onPointerOver={handlePointerOver}
     onPointerOut={handlePointerOut}
   />
   ```
   - Single draw call for all markers
   - Per-instance colors and transforms
   - Click/hover via instanceId

2. **GPU-Based Heatmap** (TODO)
   - Use WebGL fragment shader instead of canvas
   - Real-time updates without CPU processing

3. **Marker Clustering** (TODO)
   - Show detailed markers when zoomed in
   - Show aggregated/clustered markers when zoomed out
   - Use supercluster or similar library

4. **Octree/Spatial Indexing** (TODO)
   - Only render visible markers
   - Frustum culling for better performance

5. **Web Workers** (TODO)
   - Offload data processing to workers
   - Keep UI thread responsive

### When to Switch

**Consider Deck.gl if:**
- Requirements change to focus on 2D geographic heatmaps
- Working with millions of data points
- Don't need custom 3D models

**Consider Babylon.js if:**
- Need advanced game engine features
- VR/AR becomes a requirement
- Need built-in physics engine

**Consider staying with R3F if:**
- Current needs are met with optimizations
- 3D model visualization is core requirement
- Team is comfortable with React patterns

---

## Implementation Roadmap

### Phase 1: Optimize Current Stack (IN PROGRESS)
1. ✅ Implement instanced mesh rendering for markers
2. ✅ Implement camera "fit to data" functionality
3. ✅ Add mock data generator for testing
4. ⬜ Add GPU-based heatmap shader
5. ⬜ Add marker clustering for dense areas

### Phase 2: Enhanced Features
1. ⬜ Timeline/playback controls
2. ⬜ Temporal heatmap animation
3. ⬜ Export capabilities (screenshots, video)
4. ⬜ Multiple map layer support
5. ⬜ Error handling for GLB loading

### Phase 3: Advanced (if needed)
1. ⬜ Evaluate performance at production scale
2. ⬜ Consider hybrid approach (Deck.gl for heatmaps + R3F for 3D models)
3. ⬜ Implement WebGL2 features
4. ⬜ Add WebWorker support for data processing
5. ⬜ Spatial indexing with frustum culling

---

## Conclusion

The current **React Three Fiber + Three.js** implementation is the right choice for this use case. It provides the best balance of:
- 3D model support for game maps
- React integration
- Flexibility for custom visualizations
- Performance potential with proper optimization
- Zero licensing costs

Focus on optimizing the current implementation rather than switching to an alternative stack. The identified performance improvements (instancing, GPU heatmaps, clustering) will address current limitations while maintaining the architectural benefits.
