import * as THREE from 'three'
import {
  panTarget,
  sphericalToZUpOffset,
  zoomAnchorTarget,
  zUpOffsetToSphericalInput,
} from '../map-camera-math'

describe('sphericalToZUpOffset / zUpOffsetToSphericalInput round-trip', () => {
  const cases: Array<[number, number, number]> = [
    [5000, Math.PI / 4, 0],
    [1234, Math.PI / 3, Math.PI / 6],
    [800, 0.01, -Math.PI / 3],
    [10000, Math.PI / 2 - 0.05, 1.2],
  ]

  it('recovers the original offset through the round trip', () => {
    for (const [radius, phi, theta] of cases) {
      const spherical = new THREE.Spherical(radius, phi, theta)
      const offset = sphericalToZUpOffset(spherical, new THREE.Vector3())

      // Z-up offset back to a Y-up vector, then to a Spherical, then forward again.
      const back = zUpOffsetToSphericalInput(offset, new THREE.Vector3())
      const recovered = new THREE.Spherical().setFromVector3(back)
      const offset2 = sphericalToZUpOffset(recovered, new THREE.Vector3())

      expect(offset2.x).toBeCloseTo(offset.x, 6)
      expect(offset2.y).toBeCloseTo(offset.y, 6)
      expect(offset2.z).toBeCloseTo(offset.z, 6)
    }
  })

  it('places phi=0 along world-up (+Z)', () => {
    const offset = sphericalToZUpOffset(new THREE.Spherical(100, 0, 0), new THREE.Vector3())
    expect(offset.z).toBeCloseTo(100, 6)
    expect(offset.x).toBeCloseTo(0, 6)
    expect(offset.y).toBeCloseTo(0, 6)
  })
})

describe('panTarget', () => {
  it('translates only in the ground (XY) plane regardless of input', () => {
    const target = new THREE.Vector3(10, 20, 30)
    panTarget(target, 0, 1000, 5, 7)
    // theta=0: right=(-1,0,0), forward=(0,1,0), panSpeed = 1000*0.001 = 1
    // dx*right = (-5,0,0), dy*forward = (0,7,0)
    expect(target.x).toBeCloseTo(10 - 5, 6)
    expect(target.y).toBeCloseTo(20 + 7, 6)
    expect(target.z).toBeCloseTo(30, 6) // never elevates
  })

  it('scales pan speed with the orbit radius', () => {
    const a = panTarget(new THREE.Vector3(), 0, 1000, 10, 0)
    const b = panTarget(new THREE.Vector3(), 0, 2000, 10, 0)
    expect(Math.abs(b.x)).toBeCloseTo(Math.abs(a.x) * 2, 6)
  })
})

describe('zoomAnchorTarget', () => {
  it('leaves the target fixed when it equals the anchor', () => {
    const anchor = new THREE.Vector3(3, 4, 5)
    const target = anchor.clone()
    zoomAnchorTarget(target, anchor, 0.5)
    expect(target.x).toBeCloseTo(3, 6)
    expect(target.y).toBeCloseTo(4, 6)
    expect(target.z).toBeCloseTo(5, 6)
  })

  it('scales the target around the anchor by s', () => {
    const anchor = new THREE.Vector3(0, 0, 0)
    const target = new THREE.Vector3(10, 0, 0)
    zoomAnchorTarget(target, anchor, 0.5)
    expect(target.x).toBeCloseTo(5, 6)
  })
})
