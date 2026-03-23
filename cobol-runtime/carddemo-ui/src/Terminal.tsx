// 3270 Terminal Emulator Component
// 80x24 monospace character grid with field-level input

import { useState, useEffect, useCallback, useRef } from 'react';
import type { ScreenField, ScreenResponse, AidKey } from './types';
import { COLORS } from './types';

const ROWS = 24;
const COLS = 80;
const CHAR_W = 9.6;
const CHAR_H = 20;

interface TerminalProps {
  screen: ScreenResponse | null;
  onSubmit: (aid: AidKey, fields: Record<string, string>) => void;
  statusLine?: string;
}

interface FieldState {
  field: ScreenField;
  value: string;
}

export function Terminal({ screen, onSubmit, statusLine }: TerminalProps) {
  const [fieldStates, setFieldStates] = useState<Map<string, FieldState>>(new Map());
  const [cursorField, setCursorField] = useState<string | null>(null);
  const [cursorPos, setCursorPos] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);

  // Update field states when screen changes
  useEffect(() => {
    if (!screen) return;
    const states = new Map<string, FieldState>();
    for (const f of screen.fields) {
      states.set(f.name, { field: f, value: f.value });
    }
    setFieldStates(states);
    // Set initial cursor to first unprotected field
    const firstInput = screen.fields.find(f => !f.protected);
    if (firstInput) {
      setCursorField(screen.cursor_field || firstInput.name);
      setCursorPos(0);
    }
  }, [screen]);

  // Collect modified field values
  const getFieldValues = useCallback((): Record<string, string> => {
    const vals: Record<string, string> = {};
    for (const [name, state] of fieldStates) {
      if (!state.field.protected) {
        vals[name] = state.value;
      }
    }
    return vals;
  }, [fieldStates]);

  // Tab to next unprotected field
  const tabToNext = useCallback((reverse: boolean) => {
    if (!screen) return;
    const inputs = screen.fields.filter(f => !f.protected);
    if (inputs.length === 0) return;
    const idx = inputs.findIndex(f => f.name === cursorField);
    const next = reverse
      ? (idx <= 0 ? inputs.length - 1 : idx - 1)
      : (idx >= inputs.length - 1 ? 0 : idx + 1);
    setCursorField(inputs[next].name);
    setCursorPos(0);
  }, [screen, cursorField]);

  // Handle keyboard
  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    // Function keys → AID
    const fkeyMap: Record<string, AidKey> = {
      Enter: 'ENTER', Escape: 'CLEAR',
      F1: 'PF1', F2: 'PF2', F3: 'PF3', F4: 'PF4',
      F5: 'PF5', F6: 'PF6', F7: 'PF7', F8: 'PF8',
      F9: 'PF9', F10: 'PF10', F11: 'PF11', F12: 'PF12',
    };

    if (fkeyMap[e.key]) {
      e.preventDefault();
      onSubmit(fkeyMap[e.key], getFieldValues());
      return;
    }

    if (e.key === 'Tab') {
      e.preventDefault();
      tabToNext(e.shiftKey);
      return;
    }

    // Typing into current field
    if (!cursorField) return;
    const state = fieldStates.get(cursorField);
    if (!state || state.field.protected) return;

    if (e.key === 'Backspace') {
      e.preventDefault();
      if (cursorPos > 0) {
        const newVal = state.value.slice(0, cursorPos - 1) + state.value.slice(cursorPos);
        setFieldStates(prev => {
          const next = new Map(prev);
          next.set(cursorField, { ...state, value: newVal });
          return next;
        });
        setCursorPos(cursorPos - 1);
      }
      return;
    }

    if (e.key === 'ArrowLeft') { setCursorPos(Math.max(0, cursorPos - 1)); return; }
    if (e.key === 'ArrowRight') { setCursorPos(Math.min(state.field.length, cursorPos + 1)); return; }

    // Printable character
    if (e.key.length === 1 && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      // Numeric field validation
      if (state.field.numeric && !/[0-9.\-+]/.test(e.key)) return;
      // Length check
      if (state.value.length >= state.field.length && cursorPos >= state.value.length) return;

      const newVal = state.value.slice(0, cursorPos) + e.key + state.value.slice(cursorPos + 1);
      setFieldStates(prev => {
        const next = new Map(prev);
        next.set(cursorField, { ...state, value: newVal.slice(0, state.field.length) });
        return next;
      });
      setCursorPos(Math.min(cursorPos + 1, state.field.length - 1));
    }
  }, [cursorField, cursorPos, fieldStates, getFieldValues, onSubmit, tabToNext]);

  // Render
  const renderField = (name: string, state: FieldState) => {
    const { field } = state;
    const isCursor = name === cursorField;
    const color = field.dark ? '#000' : (field.bright ? '#fff' : COLORS[field.color] || COLORS.green);
    const bgColor = field.protected ? 'transparent' : 'rgba(0,80,0,0.3)';
    const displayVal = state.value.padEnd(field.length, field.protected ? ' ' : '_');

    return (
      <div
        key={name}
        onClick={() => { if (!field.protected) { setCursorField(name); setCursorPos(0); } }}
        style={{
          position: 'absolute',
          left: (field.col - 1) * CHAR_W,
          top: (field.row - 1) * CHAR_H,
          width: field.length * CHAR_W,
          height: CHAR_H,
          color,
          backgroundColor: bgColor,
          fontFamily: '"IBM Plex Mono", "Courier New", monospace',
          fontSize: '14px',
          lineHeight: `${CHAR_H}px`,
          letterSpacing: '0.5px',
          whiteSpace: 'pre',
          cursor: field.protected ? 'default' : 'text',
          outline: isCursor ? '1px solid rgba(0,255,0,0.5)' : 'none',
          overflow: 'hidden',
        }}
      >
        {displayVal}
        {isCursor && !field.protected && (
          <span style={{
            position: 'absolute',
            left: cursorPos * CHAR_W,
            top: 0,
            width: CHAR_W,
            height: CHAR_H,
            backgroundColor: 'rgba(0,255,0,0.7)',
            animation: 'blink 1s step-end infinite',
          }} />
        )}
      </div>
    );
  };

  return (
    <div
      ref={containerRef}
      tabIndex={0}
      onKeyDown={handleKeyDown}
      style={{
        position: 'relative',
        width: COLS * CHAR_W,
        height: (ROWS + 1) * CHAR_H,
        backgroundColor: '#000',
        margin: '20px auto',
        border: '2px solid #333',
        borderRadius: '4px',
        padding: '4px',
        outline: 'none',
      }}
    >
      {/* Field layer */}
      {Array.from(fieldStates.entries()).map(([name, state]) =>
        renderField(name, state)
      )}

      {/* Status line (row 25) */}
      <div style={{
        position: 'absolute',
        left: 0,
        top: ROWS * CHAR_H,
        width: COLS * CHAR_W,
        height: CHAR_H,
        color: '#888',
        fontFamily: '"IBM Plex Mono", "Courier New", monospace',
        fontSize: '12px',
        lineHeight: `${CHAR_H}px`,
        paddingLeft: 4,
        borderTop: '1px solid #333',
      }}>
        {statusLine || (screen ? `MAP: ${screen.map}  MAPSET: ${screen.mapset}` : 'No screen')}
      </div>

      <style>{`
        @keyframes blink {
          0%, 100% { opacity: 1; }
          50% { opacity: 0; }
        }
      `}</style>
    </div>
  );
}
