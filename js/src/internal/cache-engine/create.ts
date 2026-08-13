import { PurposeBuiltCacheEngine } from './engine.js';
import { cacheIndexKey } from './helpers.js';
import type { CacheEngine, CacheEngineOptions } from './types.js';

/** Create the selected private cache-engine implementation. */
export function createCacheEngine(options: CacheEngineOptions = {}): CacheEngine {
	return new PurposeBuiltCacheEngine(options);
}

export { cacheIndexKey };
