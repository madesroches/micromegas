import { useEffect, useRef, type ReactNode } from 'react'
import Navbar from './components/Navbar'
import Hero from './components/Hero'
import HowItWorks from './components/HowItWorks'
import Differentiators from './components/Differentiators'
import Notebooks from './components/Notebooks'
import Integrations from './components/Integrations'
import Footer from './components/Footer'

function FadeIn({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = ref.current
    if (!el) return
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          el.classList.add('opacity-100', 'translate-y-0')
          el.classList.remove('opacity-0', 'translate-y-8')
          observer.unobserve(el)
        }
      },
      { threshold: 0.1 },
    )
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  return (
    <div
      ref={ref}
      className="opacity-0 translate-y-8 transition-all duration-700 ease-out"
    >
      {children}
    </div>
  )
}

export default function App() {
  return (
    <div className="min-h-screen bg-app-bg">
      <Navbar />
      <main>
        <Hero />
        <FadeIn><HowItWorks /></FadeIn>
        <FadeIn><Differentiators /></FadeIn>
        <FadeIn><Notebooks /></FadeIn>
        <FadeIn><Integrations /></FadeIn>
      </main>
      <FadeIn><Footer /></FadeIn>
    </div>
  )
}
