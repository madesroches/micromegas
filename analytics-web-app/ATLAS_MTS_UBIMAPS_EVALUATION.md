# Atlas MTS and Ubimaps Evaluation for Micromegas Map Visualization

## Executive Summary

This document evaluates Ubisoft's **Atlas MTS (Map Tile Server)** and **Ubimaps** solutions for potential integration with the Micromegas analytics web application's map visualization feature.

**Key Findings:**
- ✅ MTS provides enterprise-grade map tile serving with versioning
- ✅ Ubimaps offers geospatial object storage and retrieval
- ⚠️ Both solutions are Ubisoft-internal and may require infrastructure access
- ⚠️ Integration complexity depends on data format compatibility
- ❌ **Atlas web app is a separate TypeScript application that cannot be easily integrated into Micromegas**
- ❌ **Any integration would be API-based only, not component-level**

---

## Overview of Solutions

### Important Architectural Constraint

**Atlas is a separate application:** The Atlas web application is developed using TypeScript but lives in its own standalone application. It **cannot be easily integrated** into the Micromegas analytics web app as a component library or module.

**Integration Implications:**
- ❌ Cannot import Atlas React components into Micromegas
- ❌ Cannot share UI code or visualization components
- ⚠️ Integration must be API-based (HTTP endpoints only)
- ⚠️ Could embed via iframe but with significant UX limitations
- ✅ Can consume MTS tiles via standard tile server URLs
- ✅ Can call Ubimaps REST API endpoints

This architectural separation significantly impacts integration feasibility and reinforces the recommendation to keep Micromegas' current implementation independent.

---

### Map Tile Server (MTS)

**Type:** Map tile serving infrastructure with CLI management tool

**Purpose:** Hosts and serves pre-rendered map tiles for efficient web delivery

**Key Components:**
- **MTS CLI**: Command-line tool for managing maps, layers, and tile versions
- **MTS API**: RESTful API for tile retrieval and management
- **Tile Storage**: Versioned storage for map tiles at multiple zoom levels

**Architecture Pattern:**
```
Game Assets/3D Map
    ↓ (process into tiles)
MTS CLI
    ↓ (upload)
MTS Server (Cloud/On-Prem)
    ↓ (HTTP tile requests)
Web Frontend (Leaflet/Mapbox/etc.)
```

---

### Ubimaps

**Type:** Geospatial object repository with SDK

**Purpose:** Store, query, and retrieve geo-localized objects and data

**Key Components:**
- **Ubimaps .NET SDK**: Client library for data import/export
- **HTTP API**: RESTful endpoints for CRUD operations on geo objects
- **Hierarchical Collections**: Organize geospatial data into logical groups
- **Regional Queries**: Fetch objects by geographic region/bounds

**Architecture Pattern:**
```
Game/Analytics Data
    ↓
Ubimaps .NET SDK
    ↓
Ubimaps Server (Cloud/On-Prem)
    ↓ (regional queries)
Analytics Application
```

---

## MTS (Map Tile Server) Evaluation

### What MTS Provides

Based on the documentation summary, MTS offers:

1. **Tile Management**
   - Create and register maps with metadata (size, origin, orientation)
   - Organize maps into layers
   - Version control for tile updates

2. **CLI Operations**
   - `mts-cli map create` - Register new maps
   - `mts-cli layer create` - Create map layers
   - `mts-cli layer-version create` - Upload tile versions
   - Configuration via appsettings.json, environment variables, or CLI args

3. **API Integration**
   - RESTful API for programmatic access
   - Authentication via Client ID/Secret
   - Project-based isolation (Project Key)

### Pros

#### Infrastructure & Operations
- ✅ **Enterprise-Grade**: Built for large-scale game production environments
- ✅ **Versioning**: Track map updates over time (useful for game development cycles)
- ✅ **Multi-Layer Support**: Composite multiple data layers (base map + overlays)
- ✅ **CLI Automation**: Scriptable tile upload and management
- ✅ **Authentication**: Secure access with client credentials

#### Performance
- ✅ **Tile-Based Delivery**: Standard web mapping approach (efficient caching)
- ✅ **CDN-Ready**: Tiles can be cached at edge for low latency
- ✅ **Scalable**: Designed for high-traffic game services

#### Integration
- ✅ **Standard Tile Format**: Compatible with Leaflet, Mapbox GL, OpenLayers
- ✅ **HTTP API**: Easy integration from any web client
- ✅ **No Client SDK Required**: Use standard map libraries

#### Game Industry Alignment
- ✅ **Game Map Focused**: Designed for game world visualization
- ✅ **Custom Coordinate Systems**: Supports non-geographic origins and orientations
- ✅ **Ubisoft Production-Tested**: Used by AAA game teams

### Cons

#### Infrastructure Requirements
- ❌ **Self-Hosted or Ubisoft Cloud**: Requires MTS server deployment
- ❌ **Authentication Setup**: Need Client ID/Secret provisioning
- ❌ **Project Key Required**: Must be part of Atlas ecosystem

#### Data Preparation
- ❌ **Pre-Processing Required**: Maps must be tiled before upload
- ❌ **Tile Generation**: Need external tools to convert 3D maps to tiles
- ❌ **Storage Overhead**: Tiles at multiple zoom levels consume significant storage

#### Integration Complexity
- ❌ **Static Tiles Only**: Cannot dynamically render from 3D models in browser
- ❌ **Update Workflow**: Changes require re-tiling and re-upload
- ❌ **Limited to 2D/2.5D**: Standard tile servers serve raster images, not 3D geometry

#### Vendor Lock-in
- ❌ **Ubisoft-Specific**: Not an open-source or public service
- ❌ **Internal Dependencies**: Relies on Ubisoft infrastructure
- ❌ **Limited Community**: Internal tool, no public community support
- ❌ **Separate Application**: Atlas web app cannot be integrated as component library
- ❌ **No Code Sharing**: Cannot reuse Atlas visualization components in Micromegas

#### Compatibility with Current Implementation
- ❌ **Different Paradigm**: Current R3F implementation loads 3D models (GLB/GLTF)
- ❌ **Not for Heatmap Data**: MTS serves map backgrounds, not telemetry data
- ❌ **Two-System Architecture**: Would need MTS for base map + separate system for data viz
- ❌ **No UI Code Reuse**: Cannot leverage Atlas TypeScript components in Micromegas
- ❌ **API Integration Only**: Limited to consuming tile URLs and REST endpoints

### Use Cases for MTS in Micromegas

#### ✅ Good Fit For:

1. **Pre-Rendered Game Map Backgrounds**
   - Convert 3D game maps to 2D tiles
   - Serve as base layer for analytics overlays
   - Replace GLB/GLTF loading with tile streaming

2. **Multi-Resolution Map Viewing**
   - Zoom from world view to detailed local areas
   - Progressive loading for large maps

3. **Historical Map Versioning**
   - Track map changes across game versions
   - Compare telemetry across different map layouts

#### ❌ Poor Fit For:

1. **Dynamic 3D Visualization**
   - Cannot render 3D perspective views
   - No camera rotation (tiles are top-down)

2. **Real-Time Data Overlays**
   - MTS serves static tiles, not dynamic data
   - Would need separate system for heatmaps/markers

3. **Small-Scale Projects**
   - Overhead of tile generation and server management
   - Simpler to load map images directly

### MTS Integration Architecture (if used)

```typescript
// Hypothetical integration with Leaflet or Mapbox GL
import mapboxgl from 'mapbox-gl'

const map = new mapboxgl.Map({
  container: 'map',
  style: {
    version: 8,
    sources: {
      'mts-tiles': {
        type: 'raster',
        tiles: [
          'https://mts.ubisoft.com/api/tiles/{projectKey}/{mapKey}/{z}/{x}/{y}.png'
        ],
        tileSize: 256
      }
    },
    layers: [{
      id: 'mts-layer',
      type: 'raster',
      source: 'mts-tiles'
    }]
  }
})

// Then add Deck.gl or Mapbox GL layers for death events
// ...
```

**Pros of this approach:**
- ✅ Efficient map rendering (cached tiles)
- ✅ Standard 2D mapping stack

**Cons of this approach:**
- ❌ Lose 3D capability (no GLB models)
- ❌ More complex architecture (MTS + data viz layer)
- ❌ Tile generation workflow needed

---

## Ubimaps Evaluation

### What Ubimaps Provides

Based on the documentation summary, Ubimaps offers:

1. **Geospatial Object Repository**
   - Store objects with geographic coordinates
   - Hierarchical collection organization
   - Standard HTTP CRUD operations

2. **.NET SDK**
   - Import geo-localized data
   - Query by region/bounds
   - Integration with game engines

3. **Regional Queries**
   - Fetch objects within geographic area
   - Efficient spatial indexing

### Pros

#### Data Management
- ✅ **Structured Storage**: Organize telemetry data geographically
- ✅ **Spatial Queries**: Fetch death events by map region
- ✅ **Hierarchical Organization**: Group by game session, map area, etc.
- ✅ **Standard HTTP API**: REST endpoints for integration

#### Integration with Games
- ✅ **Game Engine Support**: Designed for game data workflows
- ✅ **.NET SDK**: Easy integration with Unity/Unreal (if using C#)
- ✅ **Geo-Localized Data**: Native support for coordinate-based data

#### Analysis & Reporting
- ✅ **Built for Analytics**: Supports analysis use cases
- ✅ **Regional Aggregation**: Query specific map areas
- ✅ **Partner Integration**: API designed for external consumers

### Cons

#### Technology Stack
- ❌ **.NET Only SDK**: Official SDK is .NET (Micromegas uses Rust/Python/TypeScript)
- ❌ **HTTP API Available**: Can use REST API but lose SDK benefits
- ❌ **Artifactory Dependency**: Package hosted on Ubisoft internal Artifactory

#### Infrastructure
- ❌ **Ubisoft-Internal**: Not a public service
- ❌ **Deployment Required**: Need Ubimaps server instance
- ❌ **Authentication**: Requires Ubisoft credentials

#### Data Model Alignment
- ❌ **Geographic Focus**: Designed for lat/lng coordinates
- ❌ **Game Coordinate Mapping**: Need to map game coordinates to geographic system
- ❌ **Object-Oriented**: Not optimized for high-frequency event streams

#### Overlap with Existing Infrastructure
- ❌ **Redundant with Current Stack**: Micromegas already has PostgreSQL + object storage
- ❌ **Query Capabilities**: DataFusion already provides spatial queries
- ❌ **Additional Complexity**: Another system to maintain
- ❌ **Separate Application**: Cannot integrate Atlas UI components, API-only integration
- ❌ **Double Development**: Would need to build visualization in Micromegas anyway

### Use Cases for Ubimaps in Micromegas

#### ✅ Could Be Useful For:

1. **Standardized Geospatial API**
   - If multiple teams need access to telemetry data
   - Provides standard REST API for external consumers

2. **Cross-Game Geospatial Analysis**
   - Compare player behavior across different game maps
   - Aggregate spatial data from multiple games

3. **Integration with Other Ubisoft Tools**
   - If Atlas ecosystem integration is required
   - Share data with other internal analytics platforms

#### ❌ Not Necessary For:

1. **Current Architecture**
   - Micromegas already has efficient storage (PostgreSQL + Parquet)
   - DataFusion provides powerful query capabilities

2. **Web Visualization**
   - Frontend can query FlightSQL directly
   - No need for intermediate geospatial layer

3. **Performance**
   - Current stack handles high-frequency telemetry efficiently
   - Adding Ubimaps would add latency and complexity

### Ubimaps Integration Architecture (if used)

```typescript
// Hypothetical REST API integration
async function fetchDeathEventsByRegion(
  minX: number, minY: number,
  maxX: number, maxY: number
) {
  const response = await fetch(
    `https://ubimaps.ubisoft.com/api/projects/{projectId}/objects`,
    {
      method: 'POST',
      headers: {
        'Authorization': `Bearer ${token}`,
        'Content-Type': 'application/json'
      },
      body: JSON.stringify({
        bounds: { minX, minY, maxX, maxY },
        objectType: 'death_event'
      })
    }
  )

  return response.json()
}
```

**Pros of this approach:**
- ✅ Standardized geospatial API
- ✅ Regional query optimization

**Cons of this approach:**
- ❌ Duplicate data storage (already in Micromegas)
- ❌ Additional authentication layer
- ❌ Extra network hop (latency)
- ❌ Need to sync data from Micromegas to Ubimaps

---

## Comparison Matrix

| Feature | MTS | Ubimaps | Current R3F + Micromegas |
|---------|-----|---------|--------------------------|
| **3D Model Support** | ❌ No (2D tiles only) | ❌ No (objects only) | ✅ Yes (GLB/GLTF) |
| **Tile Serving** | ✅ Optimized | ❌ Not designed for | ⚠️ Manual (direct loading) |
| **Data Storage** | ❌ No (map tiles only) | ✅ Yes (geo objects) | ✅ Yes (PostgreSQL + Parquet) |
| **Spatial Queries** | ❌ No | ✅ Yes | ✅ Yes (DataFusion) |
| **Versioning** | ✅ Map versions | ⚠️ Object versions | ⚠️ Partition-based |
| **Performance (10K+ points)** | ⚠️ Background only | ⚠️ Depends on impl. | ⚠️ Needs optimization |
| **Infrastructure Required** | ⚠️ MTS server | ⚠️ Ubimaps server | ✅ Already deployed |
| **Integration Complexity** | Medium-High | Medium-High | ✅ Already integrated |
| **UI Component Reuse** | ❌ No (separate app) | ❌ No (separate app) | ✅ Full control |
| **Integration Type** | ⚠️ API-only | ⚠️ API-only | ✅ Native components |
| **Technology Fit** | 2D web mapping | Geospatial repository | 3D web graphics |
| **Ubisoft-Specific** | ✅ Yes | ✅ Yes | ❌ No (open source) |
| **License Cost** | Internal (free?) | Internal (free?) | ✅ Free (MIT/Apache) |

---

## Integration Scenarios

**Important Note:** All integration scenarios below assume API-level integration only. Atlas web application components cannot be directly imported or embedded into Micromegas due to architectural separation.

### Scenario 1: Hybrid - MTS for Base Map + R3F for Data

**Architecture:**
```
MTS Server (2D map tiles)
    ↓
Mapbox GL JS (base map layer)
    ↓ (overlay)
React Three Fiber (3D markers/heatmap)
```

**Pros:**
- ✅ Efficient base map loading (cached tiles)
- ✅ Keep 3D visualization capabilities
- ✅ Multi-resolution zoom support

**Cons:**
- ❌ Complex camera synchronization (2D tiles + 3D overlay)
- ❌ Requires tile generation workflow
- ❌ Need MTS infrastructure access
- ❌ Cannot reuse Atlas map viewer UI - must build custom integration
- ❌ No Atlas component sharing - API-only integration

**Recommendation:** Only if large, complex maps require tiling for performance. Note that you cannot simply embed or reuse Atlas's map viewer - you must build the Mapbox GL integration yourself in Micromegas.

---

### Scenario 2: Ubimaps for Data + Deck.gl for Visualization

**Architecture:**
```
Micromegas (telemetry ingestion)
    ↓ (sync)
Ubimaps (geospatial storage)
    ↓ (regional queries)
Web App + Deck.gl (visualization)
```

**Pros:**
- ✅ Standardized geospatial API
- ✅ Could enable other consumers

**Cons:**
- ❌ Duplicate data storage
- ❌ Sync complexity
- ❌ Added latency
- ❌ More infrastructure to maintain
- ❌ Cannot reuse Atlas's Ubimaps UI components
- ❌ Must build entire visualization layer in Micromegas anyway

**Recommendation:** Not recommended - adds complexity without clear benefit. Since Atlas components cannot be reused, you gain no UI development advantage.

---

### Scenario 3: MTS + Ubimaps (Full Atlas Stack)

**Architecture:**
```
MTS (base map tiles)
    +
Ubimaps (death event objects)
    ↓
Web App (2D web mapping)
```

**Pros:**
- ✅ Fully integrated with Atlas ecosystem
- ✅ Standardized Ubisoft workflows
- ✅ Multi-game analytics potential

**Cons:**
- ❌ Complete rewrite of current implementation
- ❌ Lose 3D visualization
- ❌ Multiple infrastructure dependencies
- ❌ Not aligned with Micromegas architecture
- ❌ **Cannot import Atlas UI code** - must rebuild visualization from scratch
- ❌ **No component reuse benefit** - API-only integration provides no UI code sharing

**Recommendation:** Not recommended. Since Atlas is a separate application, you gain no UI development benefit from this approach - you'd still need to build the entire map visualization interface in Micromegas while also depending on Atlas backend services.

---

### Scenario 4: Keep Current Stack (Recommended)

**Architecture:**
```
Micromegas (PostgreSQL + Parquet)
    ↓ (FlightSQL)
Analytics Web App (R3F + Three.js)
    ↓
GLB/GLTF Map + 3D Markers/Heatmap
```

**Pros:**
- ✅ Already working
- ✅ No additional infrastructure
- ✅ Full 3D capabilities
- ✅ Direct integration with Micromegas
- ✅ Open-source stack

**Cons:**
- ⚠️ Needs performance optimization (instancing, etc.)
- ⚠️ Manual tile generation if maps are very large

**Recommendation:** Optimize current implementation (see MAP_IMPLEMENTATION_OPTIONS.md)

---

## Decision Criteria

### Critical Constraint: No Component Integration

**Atlas web app is architecturally separate from Micromegas.** This means:
- You **cannot** import Atlas React components
- You **cannot** embed Atlas map viewers
- You **cannot** share TypeScript UI code between applications
- Integration is **API-only** (tile URLs, REST endpoints)

**Implication:** Even if you use MTS/Ubimaps backend services, you must build the entire map visualization UI in Micromegas from scratch. There is no UI development benefit from using Atlas services.

### Choose MTS + Ubimaps if:

1. ✅ **Atlas Ecosystem Required**
   - Business requirement to use Ubisoft Atlas
   - Need to share data with other Atlas-based tools

2. ✅ **Multi-Game Platform**
   - Building analytics for multiple Ubisoft games
   - Need standardized geospatial APIs

3. ✅ **2D Visualization is Sufficient**
   - Don't need 3D perspective views
   - Top-down tactical view is enough

4. ✅ **Very Large Maps**
   - Maps are so large that tiling is necessary
   - Multi-resolution zoom is critical

5. ✅ **Existing Infrastructure Access**
   - Already have MTS/Ubimaps servers deployed
   - Authentication and project keys available

6. ⚠️ **Willing to Build UI from Scratch**
   - Understand that Atlas UI components cannot be reused
   - Accept building map viewer in Micromegas despite using Atlas backend
   - Have resources to develop and maintain separate map visualization

### Keep Current Stack if:

1. ✅ **3D Visualization Required**
   - Need to load 3D game maps (GLB/GLTF)
   - Want camera rotation and perspective views

2. ✅ **Micromegas-Only Deployment**
   - Not integrating with broader Atlas ecosystem
   - Standalone observability platform

3. ✅ **Infrastructure Simplicity**
   - Want to minimize external dependencies
   - Current architecture meets needs

4. ✅ **Open Source Preference**
   - Prefer open-source stack
   - Want community support and flexibility

5. ✅ **Quick Iteration**
   - Need to iterate quickly on visualization
   - Don't want tile generation overhead

6. ✅ **UI Development Control**
   - Want full control over visualization components
   - Don't need to coordinate with Atlas team
   - Can iterate on UI without API dependencies

---

## Recommendations

### Primary Recommendation: **Optimize Current R3F Stack**

**Rationale:**
1. Already functional and integrated with Micromegas
2. Supports 3D game maps (GLB/GLTF)
3. No additional infrastructure required
4. Performance can be improved with optimization (see MAP_IMPLEMENTATION_OPTIONS.md)
5. Open-source stack with strong community support
6. **Full UI control** - No dependency on separate Atlas application
7. **No integration barrier** - All code lives in Micromegas
8. **Faster iteration** - No API coordination or separate team dependencies

**Next Steps:**
- Implement instanced rendering for markers
- Add marker clustering for dense areas
- Optimize heatmap with GPU shaders (if needed)

---

### Secondary Recommendation: **Evaluate MTS for Base Maps (if needed)**

**Consider MTS only if:**
- Game maps are extremely large (>10GB 3D assets)
- Multi-resolution tile zoom is required
- Already have access to MTS infrastructure

**Implementation:**
1. Generate 2D tiles from 3D maps using external tools
2. Upload tiles to MTS via CLI
3. Use Mapbox GL JS or Leaflet for base map
4. Overlay Deck.gl or R3F for data visualization

**Trade-offs:**
- ✅ Efficient base map rendering
- ❌ Lose 3D map viewing
- ❌ Additional infrastructure complexity
- ❌ **Must build entire UI** - Cannot reuse Atlas map viewer
- ❌ **Double development** - Build Mapbox GL integration AND maintain Atlas dependency

---

### Tertiary Recommendation: **Avoid Ubimaps for Micromegas**

**Rationale:**
- Micromegas already provides superior storage and query capabilities
- Adding Ubimaps would duplicate functionality
- Extra latency and complexity without clear benefit
- .NET SDK doesn't align with current tech stack

**Exception:**
- If there's a business requirement to expose geospatial data via standardized Ubisoft APIs for other teams/tools

**Critical Note:**
- Since Atlas is a separate application, using Ubimaps provides **no UI development benefit** for Micromegas
- You would still need to build the entire map visualization interface from scratch
- The only benefit would be data standardization for cross-team consumption

---

## Technical Feasibility Assessment

### MTS Integration Feasibility: **Medium**

**Required Work:**
1. Set up MTS server or get access to existing instance (⏱️ 1-2 days)
2. Generate map tiles from 3D assets (⏱️ 2-3 days setup, ongoing per map)
3. Implement tile layer in web app (⏱️ 3-5 days)
4. Sync camera/coordinates between tile layer and data layer (⏱️ 3-5 days)
5. Testing and optimization (⏱️ 2-3 days)

**Total Estimate:** 2-3 weeks

**Risk Factors:**
- Infrastructure access and authentication
- Tile generation pipeline complexity
- Camera synchronization issues
- **Cannot reuse Atlas UI** - Must build map viewer from scratch in Micromegas
- **Coordination overhead** - Dependency on separate Atlas team/infrastructure

---

### Ubimaps Integration Feasibility: **Low-Medium**

**Required Work:**
1. Set up Ubimaps server or get access (⏱️ 1-2 days)
2. Implement data sync from Micromegas to Ubimaps (⏱️ 3-5 days)
3. Update web app to query Ubimaps instead of FlightSQL (⏱️ 2-3 days)
4. Handle authentication and error cases (⏱️ 1-2 days)
5. Testing (⏱️ 2-3 days)

**Total Estimate:** 2 weeks

**Risk Factors:**
- Data duplication and sync reliability
- Added latency impact on UX
- Authentication complexity
- **No UI code reuse** - Must build visualization in Micromegas anyway
- **Separate application barrier** - Cannot leverage Atlas components

---

### Current Stack Optimization Feasibility: **High** ✅

**Required Work:**
1. Implement instanced rendering (⏱️ 4 hours)
2. Add marker clustering (⏱️ 1-2 days)
3. Optimize heatmap (GPU shader) (⏱️ 1-2 days, optional)
4. Add camera controls (fit-to-data, etc.) (⏱️ 4-6 hours)

**Total Estimate:** 3-5 days

**Risk Factors:**
- Minimal - optimizations are well-documented patterns

---

## Cost-Benefit Analysis

| Approach | Cost | Benefit | ROI | Notes |
|----------|------|---------|-----|-------|
| **Optimize Current R3F** | Low (3-5 days dev) | High (10x-100x perf) | ⭐⭐⭐⭐⭐ Very High | Full UI control |
| **Add MTS for Base Maps** | High (2-3 weeks + infra) | Medium (tile efficiency) | ⭐⭐ Low | Must build UI anyway |
| **Add Ubimaps** | High (2 weeks + infra) | Low (duplicate storage) | ⭐ Very Low | No UI benefit |
| **Full Atlas Stack** | Very High (4-6 weeks + infra) | Low (unless org req.) | ⭐ Very Low | Build UI from scratch |

---

## Conclusion

### Summary

**MTS and Ubimaps are enterprise solutions designed for Ubisoft's game production workflows.** They provide:
- Standardized map tile serving (MTS)
- Geospatial object repository (Ubimaps)
- Integration with Atlas ecosystem

**However, for Micromegas' use case:**
- Current R3F + Three.js stack is better aligned
- Direct integration with Micromegas is simpler and more efficient
- 3D visualization capabilities would be lost with MTS/Ubimaps
- Additional infrastructure complexity without clear benefit
- **Critical: Atlas is a separate application** - Cannot import or reuse UI components
- **API-only integration** - Must build entire map visualization in Micromegas regardless
- **No development time savings** - Using Atlas services doesn't reduce UI development work

### Final Recommendation

**✅ Optimize the current React Three Fiber implementation** rather than integrating MTS or Ubimaps.

**Rationale:**
1. Current stack supports 3D game maps (GLB/GLTF)
2. No additional infrastructure dependencies
3. Performance improvements available through optimization
4. Faster time-to-value (days vs weeks)
5. Maintains architectural simplicity
6. **Full ownership of UI code** - No dependency on separate Atlas application
7. **No integration barrier** - Atlas components cannot be reused anyway
8. **Simpler architecture** - Single application vs multi-system integration

**Exception:** Consider MTS only if:
- Game maps are multi-gigabyte 3D assets that cause browser performance issues
- Multi-resolution tile zoom is a hard requirement
- Already have MTS infrastructure available
- **AND you accept building the entire map viewer UI in Micromegas** (no Atlas component reuse)
- **AND the tile efficiency benefit outweighs the integration complexity**

**Avoid:** Ubimaps integration unless there's a specific business requirement to expose data via Ubisoft's standardized geospatial APIs. **Remember:** Using Ubimaps provides no UI development benefit since Atlas application components cannot be integrated into Micromegas.

---

## Next Steps

### Immediate (This Week)
1. ✅ Complete performance optimization of current R3F implementation
   - Implement instanced rendering (Sub-option 1A)
   - Add camera "fit to data" controls

### Short-Term (Next 2 Weeks)
2. ⚠️ Evaluate if MTS is actually needed
   - Test current implementation with production-scale maps
   - Measure load times and performance
   - Only proceed with MTS if performance is unacceptable

### Long-Term (Future)
3. ⚠️ Revisit Atlas integration if business requirements change
   - Multi-game analytics platform
   - Cross-team data sharing
   - Standardized Ubisoft tooling requirements

---

## References

### Internal Documentation
- [Atlas Home](https://confluence.ubisoft.com/display/AtlasDoc)
- [MTS (Map Tile Server)](https://confluence.ubisoft.com/pages/viewpage.action?pageId=2451864989)
- [Ubimaps](https://confluence.ubisoft.com/display/AtlasDoc/Ubimaps)
- [MTS CLI Documentation](https://confluence.ubisoft.com/display/AtlasDoc/MTS+CLI)

### Related Micromegas Documentation
- `MAP_ARCHITECTURE_EVALUATION.md` - Technology stack comparison
- `MAP_IMPLEMENTATION_OPTIONS.md` - Optimization options for current implementation

### External Resources
- [Slippy Map Tiles](https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames) - Standard tile naming convention
- [Tile Map Service](https://en.wikipedia.org/wiki/Tile_Map_Service) - TMS specification
- [Web Map Tile Service](https://www.ogc.org/standards/wmts) - WMTS standard
