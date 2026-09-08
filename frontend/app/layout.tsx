import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "z3rno graph visualizer",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
