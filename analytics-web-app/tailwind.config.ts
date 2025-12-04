import type { Config } from 'tailwindcss'

const config: Config = {
  content: [
    './src/pages/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
    extend: {
      colors: {
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
        // Custom theme colors from CSS variables
        app: {
          bg: "var(--app-bg)",
          header: "var(--header-bg)",
          sidebar: "var(--sidebar-bg)",
          panel: "var(--panel-bg)",
          card: "var(--card-bg)",
        },
        theme: {
          border: {
            DEFAULT: "var(--border-color)",
            hover: "var(--border-hover)",
          },
          text: {
            primary: "var(--text-primary)",
            secondary: "var(--text-secondary)",
            muted: "var(--text-muted)",
          },
        },
        "accent-link": {
          DEFAULT: "var(--accent-link)",
          hover: "var(--accent-link-hover)",
        },
        "accent-success": "var(--accent-success)",
        "accent-highlight": "var(--accent-highlight)",
        "accent-variable": "var(--accent-variable)",
        "accent-error": {
          DEFAULT: "var(--accent-error)",
          bright: "var(--accent-error-bright)",
        },
        "accent-warning": "var(--accent-warning)",
        // Micromegas brand colors
        brand: {
          rust: "var(--brand-rust)",
          "rust-dark": "var(--brand-rust-dark)",
          blue: "var(--brand-blue)",
          "blue-dark": "var(--brand-blue-dark)",
          gold: "var(--brand-gold)",
          "gold-dark": "var(--brand-gold-dark)",
        },
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
    },
  },
  plugins: [],
}
export default config