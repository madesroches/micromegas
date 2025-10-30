// Ensure tests run with development React (for act() support)
process.env.NODE_ENV = 'test';

// Jest setup provided by Grafana scaffolding
import './.config/jest-setup';

// Polyfill TextEncoder/TextDecoder for Node.js environment
import { TextEncoder, TextDecoder } from 'util';
global.TextEncoder = TextEncoder;
global.TextDecoder = TextDecoder;
