/** @type {import('tailwindcss').Config} */
//
// Clean, distinct palette: warm neutrals + one accent blue.
// The `indigo` and `rose` keys are kept as transitional aliases so existing
// templates continue to render until they're swept to the new tokens.
//
module.exports = {
  darkMode: 'class',
  content: [
    './templates/**/*.html',
    './assets/app.js',
  ],
  theme: {
    extend: {
      colors: {
        // ── New tokens (preferred for new code) ─────────────────────────────
        ink:   '#0F172A',
        muted: '#475569',
        line:  '#D5D8DD',
        rule:  '#E5E7EB',
        wash:  '#F4F5F7',
        paper: '#FCFCFC',
        accent: {
          DEFAULT: '#1F4FD6',
          hover:   '#173FB3',
          tint:    '#EAF0FE',
        },

        // ── Transitional aliases (so existing `bg-indigo-50` etc. still work)
        // indigo-* ramp → accent + neutrals
        indigo: {
          50:  '#EAF0FE',  // accent tint  → was teal-50
          100: '#D8E2FB',
          200: '#B6C5F6',
          300: '#8FA7EE',
          400: '#5C7EE2',
          500: '#3F66DC',
          600: '#1F4FD6',  // primary accent — was teal
          700: '#173FB3',  // accent hover
          800: '#143691',
          900: '#102B72',
          950: '#0C1E50',
        },
        // rose-* ramp → danger + tints
        rose: {
          50:  '#FEF2F2',
          100: '#FEE2E2',
          200: '#FECACA',
          300: '#FCA5A5',
          400: '#F87171',
          500: '#EF4444',
          600: '#DC2626',
          700: '#B91C1C',
          800: '#991B1B',
          900: '#7F1D1D',
        },
      },
      fontFamily: {
        // Single family for everything — clarity over cleverness
        sans:    ['"Atkinson Hyperlegible"', 'system-ui', '-apple-system', 'sans-serif'],
        display: ['"Atkinson Hyperlegible"', 'system-ui', '-apple-system', 'sans-serif'],
      },
      borderRadius: {
        DEFAULT: '6px',
        sm: '4px',
        md: '6px',
        lg: '8px',
        xl: '10px',
        '2xl': '12px',
      },
      boxShadow: {
        soft: '0 1px 2px rgba(15, 23, 42, .04)',
        pop:  '0 4px 16px rgba(15, 23, 42, .06)',
        plate:'0 16px 48px rgba(15, 23, 42, .12), 0 2px 8px rgba(15, 23, 42, .06)',
        // Transitional aliases — same restrained shadow under old class names
        picnic:      '0 1px 2px rgba(15, 23, 42, .04)',
        'picnic-md': '0 2px 8px rgba(15, 23, 42, .05)',
        'picnic-lg': '0 4px 16px rgba(15, 23, 42, .06)',
      },
    },
  },
  plugins: [],
};
