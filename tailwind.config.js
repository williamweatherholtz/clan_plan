/** @type {import('tailwindcss').Config} */
module.exports = {
  darkMode: 'class',
  content: [
    './templates/**/*.html',
    './assets/app.js',
  ],
  theme: {
    extend: {
      colors: {
        indigo: {
          50:  '#F0FDFA',
          100: '#CCFBF1',
          200: '#99F6E4',
          300: '#5EEAD4',
          400: '#2DD4BF',
          500: '#14B8A6',
          600: '#0D9488',
          700: '#0F766E',
          800: '#115E59',
          900: '#134E4A',
          950: '#042F2E',
        },
        rose: {
          50:  '#FFF1F2',
          100: '#FFE4E6',
          200: '#FECDD3',
          300: '#FDA4AF',
          400: '#FB7185',
          500: '#F43F5E',
          600: '#E11D48',
          700: '#BE123C',
          800: '#9F1239',
          900: '#881337',
        },
      },
      fontFamily: {
        display: ['"Plus Jakarta Sans"', 'system-ui', 'sans-serif'],
        sans:    ['"Nunito"', 'system-ui', 'sans-serif'],
      },
      boxShadow: {
        picnic:      '0 1px 4px rgba(13,148,136,0.10)',
        'picnic-md': '0 4px 14px rgba(13,148,136,0.12)',
        'picnic-lg': '0 8px 28px rgba(13,148,136,0.15)',
      },
    },
  },
  plugins: [],
};
