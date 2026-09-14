import type { Config } from "tailwindcss";

const config: Config = {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        surface: { DEFAULT: "#FFFFFF", subtle: "#FAFBFC", muted: "#F3F4F6", hover: "#EDEEF3" },
        ink: { DEFAULT: "#292A30", secondary: "#62646F", muted: "#797C87" },
        line: { DEFAULT: "#EAEBEF", strong: "#DADCE3" },
        brand: { DEFAULT: "#6258E8", 50:"#F5F4FF",100:"#EFEDFF",200:"#DFDBFF",300:"#BBB4FA",400:"#766BED",500:"#6258E8",600:"#5348D1",700:"#463CB1",800:"#3C348F",900:"#342D74",950:"#252052" },
      },
      fontFamily: {
        sans: [
          "Inter",
          "-apple-system",
          "BlinkMacSystemFont",
          "Segoe UI",
          "sans-serif",
        ],
        mono: ["JetBrains Mono", "Fira Code", "monospace"],
      },
      animation: {
        "pulse-slow": "pulse 3s cubic-bezier(0.4, 0, 0.6, 1) infinite",
        "fade-in": "fadeIn 0.2s ease-out",
        "slide-in": "slideIn 0.2s ease-out",
      },
      keyframes: {
        fadeIn: {
          "0%": { opacity: "0" },
          "100%": { opacity: "1" },
        },
        slideIn: {
          "0%": { opacity: "0", transform: "translateY(-4px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
      },
    },
  },
  plugins: [],
};

export default config;
