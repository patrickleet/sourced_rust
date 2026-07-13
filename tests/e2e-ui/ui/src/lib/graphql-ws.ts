/**
 * graphql-transport-ws client.
 * Auth: Bearer access token in connection_init (OIDC best practice for browsers).
 */

export type GqlWsHandlers = {
  onNext: (data: unknown) => void;
  onError?: (err: unknown) => void;
  onComplete?: () => void;
};

export function graphqlWsUrl(path = '/graphql/ws'): string {
  if (typeof window === 'undefined') return path;
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  // Prefer same-origin so Vite proxies WS in dev.
  return `${proto}//${window.location.host}${path.startsWith('/') ? path : `/${path}`}`;
}

export function subscribe(
  query: string,
  auth: { accessToken?: string; userId?: string; role?: string },
  handlers: GqlWsHandlers
): () => void {
  const url = new URL(graphqlWsUrl('/graphql/ws'), window.location.href);
  // DevHeaders fallback only when no Bearer (offline demos).
  if (!auth.accessToken && auth.userId) {
    url.searchParams.set('x-user-id', auth.userId);
    url.searchParams.set('x-role', auth.role ?? 'user');
  }

  const ws = new WebSocket(url.toString(), 'graphql-transport-ws');
  const opId = '1';
  let closed = false;

  ws.onopen = () => {
    const payload: Record<string, string> = {};
    if (auth.accessToken) {
      payload.authorization = `Bearer ${auth.accessToken}`;
      payload.accessToken = auth.accessToken;
    } else if (auth.userId) {
      payload['x-user-id'] = auth.userId;
      payload['x-role'] = auth.role ?? 'user';
    }
    ws.send(JSON.stringify({ type: 'connection_init', payload }));
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
            payload: { query }
          })
        );
        break;
      case 'next':
        handlers.onNext(msg.payload);
        break;
      case 'error':
        handlers.onError?.(msg.payload ?? 'subscription error');
        break;
      case 'complete':
        handlers.onComplete?.();
        break;
      case 'ping':
        ws.send(JSON.stringify({ type: 'pong' }));
        break;
      case 'connection_error':
        handlers.onError?.(msg.payload ?? 'connection_error');
        break;
      default:
        break;
    }
  };

  ws.onerror = (e) => handlers.onError?.(e);
  ws.onclose = (ev) => {
    if (!closed && !ev.wasClean) {
      handlers.onError?.(
        `WebSocket closed (${ev.code}${ev.reason ? `: ${ev.reason}` : ''}) — check API /graphql/ws`
      );
    }
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
