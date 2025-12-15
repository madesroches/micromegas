import '@testing-library/jest-dom'

// Set NODE_ENV to development for tests
// @ts-expect-error - Override readonly property for testing
process.env.NODE_ENV = 'development'

// Mock the config module to provide a consistent basePath for tests
jest.mock('@/lib/config', () => ({
  getConfig: () => ({ basePath: '' }),
  getLinkBasePath: () => '',
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

// Mock window.location
const locationMock: Location = {
  href: 'http://localhost:3000',
  origin: 'http://localhost:3000',
  protocol: 'http:',
  host: 'localhost:3000',
  hostname: 'localhost',
  port: '3000',
  pathname: '/',
  search: '',
  hash: '',
  assign: jest.fn(),
  reload: jest.fn(),
  replace: jest.fn(),
  ancestorOrigins: { length: 0, contains: () => false, item: () => null, [Symbol.iterator]: function* () {} },
  toString: () => 'http://localhost:3000',
}

// @ts-expect-error - Override readonly property for testing
delete window.location
window.location = locationMock
