import { Metadata } from 'next'

export const metadata: Metadata = {
  title: 'Process Details',
}

export default function ProcessLayout({ children }: { children: React.ReactNode }) {
  return children
}
