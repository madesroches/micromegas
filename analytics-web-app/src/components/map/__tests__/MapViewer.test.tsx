import * as THREE from 'three'
import { cameraBasisFromSpherical } from '../MapViewer'

function expectVec(v: THREE.Vector3, x: number, y: number, z: number) {
  expect(v.x).toBeCloseTo(x, 10)
  expect(v.y).toBeCloseTo(y, 10)
  expect(v.z).toBeCloseTo(z, 10)
}

describe('cameraBasisFromSpherical', () => {
  const cases: Array<[number, number]> = [
    [0, 0],
    [Math.PI / 4, Math.PI / 4],
    [Math.PI / 2, Math.PI / 3],
    [-Math.PI / 3, 0.1],
  ]

  it('returns an orthonormal basis at every tilt', () => {
    for (const [theta, phi] of cases) {
      const { right, up, forward } = cameraBasisFromSpherical(theta, phi)
      expect(right.length()).toBeCloseTo(1, 10)
      expect(up.length()).toBeCloseTo(1, 10)
      expect(forward.length()).toBeCloseTo(1, 10)
      expect(right.dot(up)).toBeCloseTo(0, 10)
      expect(right.dot(forward)).toBeCloseTo(0, 10)
      expect(up.dot(forward)).toBeCloseTo(0, 10)
    }
  })

  it('matches the top-down (phi=0) boundary case', () => {
    {
      const { right, up, forward } = cameraBasisFromSpherical(0, 0)
      expectVec(right, 1, 0, 0)
      expectVec(up, 0, 1, 0)
      expectVec(forward, 0, 0, -1)
    }
    {
      const theta = Math.PI / 2
      const { right, up, forward } = cameraBasisFromSpherical(theta, 0)
      expectVec(right, Math.cos(theta), Math.sin(theta), 0)
      expectVec(up, -Math.sin(theta), Math.cos(theta), 0)
      expectVec(forward, 0, 0, -1)
    }
  })

  it('keeps up near world-up and forward near XY at the near-horizontal limit', () => {
    const phi = Math.PI / 2 - 0.05
    const { right, up, forward } = cameraBasisFromSpherical(0, phi)
    expectVec(right, 1, 0, 0)
    expect(up.z).toBeCloseTo(Math.sin(phi), 10)
    expect(up.z).toBeGreaterThan(0.99)
    expect(forward.x).toBeCloseTo(0, 10)
    expect(forward.y).toBeCloseTo(Math.sin(phi), 10)
    expect(forward.z).toBeCloseTo(-Math.cos(phi), 10)
    // up and forward remain orthogonal — the original bug had them collapse.
    expect(up.dot(forward)).toBeCloseTo(0, 10)
  })

  it('keeps right horizontal for any (theta, phi)', () => {
    for (const [theta, phi] of cases) {
      const { right } = cameraBasisFromSpherical(theta, phi)
      expect(right.z).toBe(0)
    }
  })

  it('is right-handed: up cross right equals forward', () => {
    const { right, up, forward } = cameraBasisFromSpherical(Math.PI / 3, Math.PI / 4)
    const crossed = new THREE.Vector3().crossVectors(up, right)
    expectVec(crossed, forward.x, forward.y, forward.z)
  })

  it('produces finite values at the clamp limits', () => {
    for (const [theta, phi] of [
      [0, 0],
      [0, Math.PI / 2 - 0.05],
    ] as Array<[number, number]>) {
      const { right, up, forward } = cameraBasisFromSpherical(theta, phi)
      for (const v of [right, up, forward]) {
        expect(Number.isFinite(v.x)).toBe(true)
        expect(Number.isFinite(v.y)).toBe(true)
        expect(Number.isFinite(v.z)).toBe(true)
      }
    }
  })
})
