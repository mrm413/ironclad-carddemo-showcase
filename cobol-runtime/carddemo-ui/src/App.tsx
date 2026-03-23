// CardDemo 3270 Terminal Application
// Connects to CICS Web API and renders interactive 3270 screens

import { useState, useCallback, useEffect } from 'react';
import { Terminal } from './Terminal';
import { createSession, sendScreen } from './api';
import type { ScreenResponse, AidKey } from './types';

export function App() {
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [screen, setScreen] = useState<ScreenResponse | null>(null);
  const [status, setStatus] = useState('Connecting...');
  const [error, setError] = useState<string | null>(null);

  // Initialize session
  useEffect(() => {
    async function init() {
      try {
        const session = await createSession('SIGN', 'GUEST');
        setSessionId(session.session_id);
        setStatus(`Session: ${session.session_id}`);
        // Send initial ENTER to get first screen
        const resp = await sendScreen({
          session_id: session.session_id,
          aid: 'ENTER',
          fields: {},
        });
        setScreen(resp);
        setError(null);
      } catch (e) {
        setError(`Connection failed: ${e}`);
        setStatus('Disconnected');
      }
    }
    init();
  }, []);

  // Handle screen submission
  const handleSubmit = useCallback(async (aid: AidKey, fields: Record<string, string>) => {
    if (!sessionId) return;
    setStatus(`Sending ${aid}...`);
    try {
      const resp = await sendScreen({
        session_id: sessionId,
        aid,
        fields,
      });
      setScreen(resp);
      setStatus(`Session: ${sessionId}`);
      setError(null);

      // Sound alarm if requested
      if (resp.alarm) {
        try { new Audio('data:audio/wav;base64,UklGRl9vT19XQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQ==').play(); }
        catch {}
      }
    } catch (e) {
      setError(`Send failed: ${e}`);
    }
  }, [sessionId]);

  return (
    <div style={{
      display: 'flex',
      flexDirection: 'column',
      alignItems: 'center',
      minHeight: '100vh',
      backgroundColor: '#000',
      color: '#0f0',
      fontFamily: '"IBM Plex Mono", "Courier New", monospace',
    }}>
      {/* Header */}
      <div style={{
        padding: '10px 20px',
        fontSize: '14px',
        color: '#888',
        textAlign: 'center',
        width: '100%',
      }}>
        CardDemo CICS Terminal | Tab: next field | Shift+Tab: prev | Enter: submit | F3: exit | Esc: clear
      </div>

      {/* Error banner */}
      {error && (
        <div style={{
          padding: '8px 20px',
          backgroundColor: '#400',
          color: '#f88',
          fontSize: '13px',
          width: '100%',
          textAlign: 'center',
        }}>
          {error}
        </div>
      )}

      {/* Terminal */}
      <Terminal
        screen={screen}
        onSubmit={handleSubmit}
        statusLine={status}
      />

      {/* Key legend */}
      <div style={{
        display: 'flex',
        gap: '16px',
        padding: '10px',
        fontSize: '11px',
        color: '#666',
        flexWrap: 'wrap',
        justifyContent: 'center',
      }}>
        {[
          ['Enter', 'Submit'], ['F1', 'Help'], ['F3', 'Exit'],
          ['F7', 'Page Up'], ['F8', 'Page Down'],
          ['Tab', 'Next Field'], ['Esc', 'Clear'],
        ].map(([key, desc]) => (
          <span key={key}>
            <span style={{ color: '#0a0', border: '1px solid #333', padding: '1px 4px', borderRadius: 2 }}>
              {key}
            </span>{' '}{desc}
          </span>
        ))}
      </div>
    </div>
  );
}
