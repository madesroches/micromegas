interface MicromegasLogoProps {
  className?: string
  showText?: boolean
  size?: 'sm' | 'md' | 'lg'
}

export function MicromegasLogo({ className = '', showText = true, size = 'md' }: MicromegasLogoProps) {
  const sizeConfig = {
    sm: { icon: 24, text: 14, gap: 6 },
    md: { icon: 32, text: 18, gap: 8 },
    lg: { icon: 48, text: 24, gap: 12 },
  }

  const { icon, text, gap } = sizeConfig[size]

  return (
    <div className={`flex items-center ${className}`} style={{ gap }}>
      <svg
        viewBox="0 0 100 100"
        width={icon}
        height={icon}
        xmlns="http://www.w3.org/2000/svg"
      >
        <defs>
          <linearGradient id="ring1" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#bf360c" />
            <stop offset="100%" stopColor="#8d3a14" />
          </linearGradient>
          <linearGradient id="ring2" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#1565c0" />
            <stop offset="100%" stopColor="#0d47a1" />
          </linearGradient>
          <linearGradient id="ring3" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#ffc107" />
            <stop offset="100%" stopColor="#ffb300" />
          </linearGradient>
          <filter id="glow" x="-50%" y="-50%" width="200%" height="200%">
            <feGaussianBlur stdDeviation="1" result="coloredBlur" />
            <feMerge>
              <feMergeNode in="coloredBlur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>
        <g transform="translate(50, 50)">
          <ellipse
            cx="0"
            cy="0"
            rx="42"
            ry="16"
            fill="none"
            stroke="url(#ring1)"
            strokeWidth="2.5"
            transform="rotate(-20)"
            opacity="0.9"
          />
          <ellipse
            cx="0"
            cy="0"
            rx="33"
            ry="13"
            fill="none"
            stroke="url(#ring2)"
            strokeWidth="2.5"
            transform="rotate(25)"
            opacity="0.9"
          />
          <ellipse
            cx="0"
            cy="0"
            rx="24"
            ry="9"
            fill="none"
            stroke="url(#ring3)"
            strokeWidth="2.5"
            transform="rotate(-8)"
            opacity="0.9"
          />
          <g filter="url(#glow)">
            <polygon
              points="0,-6 1.2,-2 4.5,-2 2,0.5 3,4.5 0,2 -3,4.5 -2,0.5 -4.5,-2 -1.2,-2"
              fill="#ffffff"
            />
          </g>
        </g>
      </svg>
      {showText && (
        <span
          className="font-light tracking-widest text-white"
          style={{ fontSize: text }}
        >
          micromegas
        </span>
      )}
    </div>
  )
}
