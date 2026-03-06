import '@testing-library/jest-dom'
import { TextEncoder, TextDecoder } from 'util'
import { ReadableStream, TransformStream, WritableStream } from 'stream/web'

// Polyfill TextEncoder/TextDecoder for Apache Arrow
global.TextEncoder = TextEncoder
// @ts-expect-error - TextDecoder type mismatch
global.TextDecoder = TextDecoder

// Polyfill Web Streams API
// @ts-expect-error - Type mismatch between Node and Web APIs
global.ReadableStream = ReadableStream
// @ts-expect-error - Type mismatch between Node and Web APIs
global.TransformStream = TransformStream
// @ts-expect-error - Type mismatch between Node and Web APIs
global.WritableStream = WritableStream

// Polyfill DOMRect for Radix UI context menu positioning
if (typeof globalThis.DOMRect === 'undefined') {
  // @ts-expect-error - Minimal DOMRect polyfill for jsdom
  globalThis.DOMRect = class DOMRect {
    x: number; y: number; width: number; height: number
    constructor(x = 0, y = 0, width = 0, height = 0) {
      this.x = x; this.y = y; this.width = width; this.height = height
    }
    get top() { return this.y }
    get left() { return this.x }
    get bottom() { return this.y + this.height }
    get right() { return this.x + this.width }
    toJSON() { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, left: this.left, bottom: this.bottom, right: this.right } }
    static fromRect(rect?: { x?: number; y?: number; width?: number; height?: number }) {
      return new DOMRect(rect?.x, rect?.y, rect?.width, rect?.height)
    }
  }
}

// Set NODE_ENV to development for tests
// @ts-expect-error - Override readonly property for testing
process.env.NODE_ENV = 'development'

// Mock the config module to provide a consistent basePath for tests
jest.mock('@/lib/config', () => ({
  getConfig: () => ({ basePath: '' }),
  appLink: (path: string) => path,
}))

// Default mock for react-router-dom (can be overridden in individual tests)
const mockNavigate = jest.fn()

jest.mock('react-router-dom', () => ({
  ...jest.requireActual('react-router-dom'),
  useNavigate: jest.fn(() => mockNavigate),
  useLocation: jest.fn(() => ({ pathname: '/', search: '', hash: '', state: null, key: 'default' })),
  useSearchParams: jest.fn(() => [new URLSearchParams(), jest.fn()]),
}))

// window.location is configured via jest testEnvironmentOptions.url = 'http://localhost:3000'
// In jsdom 26+, window.location cannot be replaced. Use jest.spyOn in individual tests.
