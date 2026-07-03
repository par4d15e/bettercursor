/** @type {import('tailwindcss').Config} */
export default {
  darkMode: 'class',
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        // cc-switch-style dark theme
        bg: {
          primary: '#0a0a0a',
          secondary: '#141414',
          tertiary: '#1e1e1e',
          hover: '#262626',
        },
        border: {
          DEFAULT: '#262626',
          strong: '#404040',
        },
        fg: {
          primary: '#e5e5e5',
          secondary: '#a3a3a3',
          muted: '#737373',
        },
        accent: {
          blue: '#3b82f6',
          green: '#22c55e',
          purple: '#a855f7',
          red: '#ef4444',
        },
      },
      fontFamily: {
        sans: [
          '-apple-system',
          'BlinkMacSystemFont',
          'system-ui',
          'Roboto',
          'Helvetica Neue',
          'Arial',
          'sans-serif',
        ],
        mono: [
          'ui-monospace',
          'SFMono-Regular',
          'Menlo',
          'Monaco',
          'Consolas',
          'monospace',
        ],
      },
    },
  },
  plugins: [],
};
