// API client for CICS Web runtime

import type { ScreenRequest, ScreenResponse, SessionResponse } from './types';

const BASE_URL = '/api';

export async function createSession(
  transid?: string,
  userid?: string,
): Promise<SessionResponse> {
  const res = await fetch(`${BASE_URL}/session`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ transid, userid }),
  });
  if (!res.ok) throw new Error(`Session creation failed: ${res.status}`);
  return res.json();
}

export async function sendScreen(req: ScreenRequest): Promise<ScreenResponse> {
  const res = await fetch(`${BASE_URL}/screen`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(req),
  });
  if (!res.ok) throw new Error(`Screen POST failed: ${res.status}`);
  return res.json();
}

export async function getScreen(sessionId: string): Promise<ScreenResponse> {
  const res = await fetch(`${BASE_URL}/screen?session_id=${encodeURIComponent(sessionId)}`);
  if (!res.ok) throw new Error(`Screen GET failed: ${res.status}`);
  return res.json();
}
