import { useState } from 'react'
import { Copy, Check } from 'lucide-react'

interface CopyableProcessIdProps {
  processId: string
  className?: string
  showIcon?: boolean
  truncate?: boolean
}

export function CopyableProcessId({ 
  processId, 
  className = '', 
  showIcon = true,
  truncate = false
}: CopyableProcessIdProps) {
  const [copied, setCopied] = useState(false)

  const handleCopy = async (e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    
    try {
      await navigator.clipboard.writeText(processId)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch (error) {
      console.error('Failed to copy process ID:', error)
      // Fallback for older browsers
      const textArea = document.createElement('textarea')
      textArea.value = processId
      document.body.appendChild(textArea)
      textArea.select()
      document.execCommand('copy')
      document.body.removeChild(textArea)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    }
  }

  const displayText = truncate ? `${processId.substring(0, 8)}...` : processId

  return (
    <span 
      className={`inline-flex items-center gap-1 cursor-pointer hover:bg-gray-100 rounded px-1 py-0.5 transition-colors ${className}`}
      onClick={handleCopy}
title={processId}
    >
      <span className="font-mono text-sm select-all">
        {displayText}
      </span>
      {showIcon && (
        <span className="opacity-50 hover:opacity-100 transition-opacity">
          {copied ? (
            <Check className="w-3 h-3 text-green-600" />
          ) : (
            <Copy className="w-3 h-3" />
          )}
        </span>
      )}
    </span>
  )
}