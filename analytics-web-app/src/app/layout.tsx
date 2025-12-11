import type { Metadata } from 'next'
import { Inter } from 'next/font/google'
import './globals.css'
import { QueryProvider } from '@/components/QueryProvider'
import { Toaster } from '@/components/ui/toaster'
import { AuthProvider } from '@/lib/auth'

const inter = Inter({ subsets: ['latin'] })

// Base path for assets (set at build time via NEXT_PUBLIC_BASE_PATH)
const basePath = process.env.NEXT_PUBLIC_BASE_PATH || ''

export const metadata: Metadata = {
  title: {
    template: 'Micromegas - %s',
    default: 'Micromegas',
  },
  description: 'Analytics web application for micromegas telemetry data',
  icons: {
    icon: `${basePath}/icon.svg`,
  },
}

export default function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <html lang="en">
      <body className={inter.className}>
        <AuthProvider>
          <QueryProvider>
            {children}
            <Toaster />
          </QueryProvider>
        </AuthProvider>
      </body>
    </html>
  )
}