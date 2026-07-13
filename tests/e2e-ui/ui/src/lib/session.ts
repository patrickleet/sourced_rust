/** Dev session: cookies stand in for Auth.js (the-website pattern, simplified). */

function readCookie(name: string): string | undefined {
  if (typeof document === 'undefined') return undefined;
  const m = document.cookie.match(new RegExp(`(?:^|; )${name}=([^;]*)`));
  return m ? decodeURIComponent(m[1]) : undefined;
}

export type Session = { userId: string; role: string };

export function readSession(): Session {
  return {
    userId: readCookie('x-user-id') ?? 'alice',
    role: readCookie('x-role') ?? 'user',
  };
}

export function writeSession(s: Session) {
  if (typeof document === 'undefined') return;
  document.cookie = `x-user-id=${encodeURIComponent(s.userId)}; path=/`;
  document.cookie = `x-role=${encodeURIComponent(s.role)}; path=/`;
}

export function identityHeaders(s: Session): Record<string, string> {
  return {
    'content-type': 'application/json',
    'x-user-id': s.userId,
    'x-role': s.role,
  };
}
