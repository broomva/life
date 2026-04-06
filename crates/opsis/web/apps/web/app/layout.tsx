import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Opsis — World State Engine",
  description: "AI-native continuous world state simulation for Life Agent OS",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="antialiased">{children}</body>
    </html>
  );
}
