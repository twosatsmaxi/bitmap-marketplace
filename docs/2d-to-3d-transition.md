# 2D→3D Transition Implementation

## Overview
A smooth morphing transition when toggling between 2D (flat) and 3D (isometric) views in the explore page. Tiles grow from flat to extruded 3D blocks over 900ms with ease-in-out easing.

---

## Key Changes

### 1. WebGLBitmapRenderer.tsx - Always use isometric shader

```typescript
// BEFORE: Switched between flat and iso programs
const prog = isometric ? shared.isoProgram : shared.program;

// AFTER: Always use isometric program, let tileHeightScale control the morph
const prog = shared.isoProgram;
const bufs = shared.isoBuffers;
const unis = shared.isoUniforms;
const vertCount = 18; // Always use cube vertices (3 faces × 6 vertices)
```

Also removed conditional depth buffer:

```typescript
// BEFORE: Toggle depth test
if (isometric) {
  gl.enable(gl.DEPTH_TEST);
} else {
  gl.disable(gl.DEPTH_TEST);
}

// AFTER: Always enable depth test
gl.enable(gl.DEPTH_TEST);
gl.depthFunc(gl.LESS);
```

### 2. shaders-iso.ts - Animate block height in vertex shader

```glsl
// Add grow factor based on uniform
float grow = smoothstep(0.0, 1.0, u_tileHeightScale);
float currentHeight = effHeight * grow;

// Use currentHeight instead of effHeight for all faces
vec3 corner;
if (faceId < 0.5) {
  corner = vec3(bx + dx * effSize, by + dy * effSize, currentHeight);
} else if (faceId < 1.5) {
  corner = vec3(bx + effSize, by + dx * effSize, dy * currentHeight);
} else {
  corner = vec3(bx + dx * effSize, by + effSize, dy * currentHeight);
}
```

### 3. Animation timing - 900ms ease-in-out

```typescript
const duration = 900; // ms
const eased = easeInOutCubic(progress);
tileHeightScaleRef.current = start + (target - start) * eased;
```

---

## Pitfalls Encountered

### 1. "Bitmap not found" / black screen
- **Cause**: Shader variable name mismatch (`isoX/isoY` renamed to `projX/projY` but screen positioning code still used old names)
- **Fix**: Ensure all references are updated when refactoring

### 2. "Too many active WebGL contexts"
- **Cause**: Attempting to recompile shaders on HMR by recreating WebGL contexts
- **Fix**: Don't recreate the shared GL context - either reload the page or just accept that shader changes require a manual refresh during dev

### 3. 3D→2D transition not working
- **Cause**: The `isometric` prop change was triggering the animation, but the `tileHeightScale` uniform wasn't being passed to the shader because the uniform location check was conditional
- **Fix**: Remove the `isometric &&` condition from the uniform check:

```typescript
// BEFORE
if (isometric && unis.u_tileHeightScale != null) {
  gl.uniform1f(unis.u_tileHeightScale, tileHeightScale);
}

// AFTER
if (unis.u_tileHeightScale != null) {
  gl.uniform1f(unis.u_tileHeightScale, tileHeightScale);
}
```

### 4. Camera rotation vs block extrusion confusion
- Initially tried rotating the "camera" from top-down to isometric, then extruding
- **Realization**: The simpler approach is better - just extrude the blocks from flat to 3D. The isometric projection is always applied; only the Z-height changes.

---

## Why It Works

1. **Single shader path**: By always using the isometric shader, we avoid the jarring switch between two different rendering pipelines

2. **Height-based morph**: The shader calculates `currentHeight = effHeight * grow` where `grow` goes from 0→1. When `grow=0`, all faces collapse to z=0 (flat). When `grow=1`, blocks have full height.

3. **Proper animation loop**: The `useEffect` watches `isometric` prop changes and animates `tileHeightScaleRef.current` from current value to target (0 or 1), triggering re-renders via `renderFrame()`

4. **900ms duration with ease-in-out**: Long enough to see the transition, smooth acceleration/deceleration feels natural

---

## Files Modified (in bitmap-marketplace-frontend)
- `src/components/explore/WebGLBitmapRenderer.tsx` - Core animation logic and shader selection
- `src/components/explore/shaders-iso.ts` - Vertex shader height animation

---

## Key Insight

The "aha" moment was realizing we should **always render with the isometric shader** and just animate the block height. The flat 2D view is simply the isometric view with zero height on all blocks. This creates a seamless morph because the projection math is identical throughout the transition.
