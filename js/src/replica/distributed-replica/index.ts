import type {
	DistributedReplica as DistributedReplicaApi,
	DistributedReplicaOptions
} from '../types.js';
import { DistributedReplicaImpl } from './impl.js';

export function createDistributedReplica(
	options: DistributedReplicaOptions = {}
): DistributedReplicaApi {
	return new DistributedReplicaImpl(options);
}
