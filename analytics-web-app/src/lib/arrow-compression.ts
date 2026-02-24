/**
 * Register LZ4 decompression codec for Arrow IPC streams.
 *
 * Import this module for side effects before reading any compressed IPC data.
 */

import { compressionRegistry, CompressionType } from 'apache-arrow';
import * as lz4js from 'lz4js';

compressionRegistry.set(CompressionType.LZ4_FRAME, {
  decode: (data: Uint8Array) => lz4js.decompress(data),
});
