// Types matching the Rust BMS screen model

export interface ScreenField {
  name: string;
  row: number;
  col: number;
  length: number;
  value: string;
  protected: boolean;
  numeric: boolean;
  bright: boolean;
  dark: boolean;
  color: string;
}

export interface ScreenResponse {
  map: string;
  mapset: string;
  fields: ScreenField[];
  cursor_field: string | null;
  erase: boolean;
  alarm: boolean;
}

export interface ScreenRequest {
  session_id: string;
  aid: string;
  fields: Record<string, string>;
  cursor_row?: number;
  cursor_col?: number;
}

export interface SessionResponse {
  session_id: string;
}

// 3270 color mapping
export const COLORS: Record<string, string> = {
  default: '#00ff00',
  green: '#00ff00',
  blue: '#5555ff',
  red: '#ff5555',
  pink: '#ff55ff',
  turquoise: '#00ffff',
  yellow: '#ffff55',
  white: '#ffffff',
};

// AID key names
export const AID_KEYS = [
  'ENTER', 'CLEAR',
  'PF1', 'PF2', 'PF3', 'PF4', 'PF5', 'PF6',
  'PF7', 'PF8', 'PF9', 'PF10', 'PF11', 'PF12',
  'PF13', 'PF14', 'PF15', 'PF16', 'PF17', 'PF18',
  'PF19', 'PF20', 'PF21', 'PF22', 'PF23', 'PF24',
  'PA1', 'PA2', 'PA3',
] as const;

export type AidKey = typeof AID_KEYS[number];
