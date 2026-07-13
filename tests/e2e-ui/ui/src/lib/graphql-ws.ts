/**
 * Minimal graphql-transport-ws client for GraphQL live queries.
 *
 * Connects same-origin to `/graphql/ws` (Vite proxies WS → API in dev).
 * Identity via query params (browsers cannot set custom WebSocket headers);
 * the server merges `x-user-id` / `x-role` into the upgrade session.
 */

export type GqlWsHandlers = {
  onNext: (data: unknown) => void;
  onError?: (err: unknown) => void;
  onComplete?: () => void;
};

export function graphqlWsUrl(path = '/graphql/ws'): string {
  if (typeof window === 'undefined') return path;
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${window.location.host}${path.startsWith('/') ? path : `/${path}`}`;
}

/** Subscribe; returns unsubscribe. */
export function subscribe(
  query: string,
  session: { userId: string; role: string },
  handlers: GqlWsHandlers
): () => void {
  const url = new URL(graphqlWsUrl('/graphql/ws'), window.location.href);
  url.searchParams.set('x-user-id', session.userId);
  url.searchParams.set('x-role', session.role);

  const ws = new WebSocket(url.toString(), 'graphql-transport-ws');
  const opId = '1';
  let closed = false;

  ws.onopen = () => {
    ws.send(JSON.stringify({ type: 'connection_init', payload: {} }));
  };

  ws.onmessage = (ev) => {
    let msg: { type: string; id?: string; payload?: unknown };
    try {
      msg = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    switch (msg.type) {
      case 'connection_ack':
        ws.send(
          JSON.stringify({
            type: 'subscribe',
            id: opId,
            payload: { query },
          })
        );
        break;
      case 'next':
        handlers.onNext(msg.payload);
        break;
      case 'error':
        handlers.onError?.(msg.payload);
        break;
      case 'complete':
        handlers.onComplete?.();
        break;
      case 'ping':
        ws.send(JSON.stringify({ type: 'pong' }));
        break;
      default:
        break;
    }
  };

  ws.onerror = (e) => handlers.onError?.(e);
  ws.onclose = () => {
    if (!closed) handlers.onComplete?.();
  };

  return () => {
    closed = true;
    if (ws.readyState === WebSocket.OPEN) {
      try {
        ws.send(JSON.stringify({ type: 'complete', id: opId }));
      } catch {
        /* ignore */
      }
    }
    ws.close();
  };
}
