'use client'

import { useEffect, useState } from 'react'
import { ProgressUpdate } from '@/types'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { Button } from '@/components/ui/button'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { Download, CheckCircle2, X } from 'lucide-react'

interface TraceGenerationProgressProps {
  isGenerating: boolean
  progress: ProgressUpdate | null
  onCancel?: () => void
  processId?: string
}

export function TraceGenerationProgress({ 
  isGenerating, 
  progress, 
  onCancel,
  processId 
}: TraceGenerationProgressProps) {
  const [startTime, setStartTime] = useState<Date | null>(null)
  const [elapsedTime, setElapsedTime] = useState(0)

  useEffect(() => {
    if (isGenerating && !startTime) {
      setStartTime(new Date())
    } else if (!isGenerating) {
      setStartTime(null)
      setElapsedTime(0)
    }
  }, [isGenerating, startTime])

  useEffect(() => {
    let interval: NodeJS.Timeout | null = null
    
    if (startTime && isGenerating) {
      interval = setInterval(() => {
        setElapsedTime(Math.floor((new Date().getTime() - startTime.getTime()) / 1000))
      }, 1000)
    }

    return () => {
      if (interval) {
        clearInterval(interval)
      }
    }
  }, [startTime, isGenerating])

  if (!isGenerating && !progress) {
    return null
  }

  const formatTime = (seconds: number) => {
    const mins = Math.floor(seconds / 60)
    const secs = seconds % 60
    return `${mins}:${secs.toString().padStart(2, '0')}`
  }

  return (
    <Card className="w-full max-w-md mx-auto">
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center justify-between text-lg">
          <span className="flex items-center gap-2">
            {isGenerating ? (
              <>
                <div className="animate-spin rounded-full h-5 w-5 border-2 border-primary border-t-transparent" />
                Generating Trace
              </>
            ) : (
              <>
                <CheckCircle2 className="w-5 h-5 text-green-600" />
                Generation Complete
              </>
            )}
          </span>
          {onCancel && isGenerating && (
            <Button 
              variant="ghost" 
              size="sm"
              onClick={onCancel}
              className="h-8 w-8 p-0"
            >
              <X className="w-4 h-4" />
            </Button>
          )}
        </CardTitle>
        {processId && (
          <p className="text-sm text-muted-foreground">
            Process: <CopyableProcessId processId={processId} className="text-sm" showIcon={false} />
          </p>
        )}
      </CardHeader>
      
      <CardContent className="space-y-4">
        {progress && (
          <>
            <div className="space-y-2">
              <div className="flex justify-between text-sm">
                <span>{progress.message}</span>
                <span>{progress.percentage}%</span>
              </div>
              <Progress value={progress.percentage} className="h-3" />
            </div>
            
            <div className="flex justify-between text-sm text-muted-foreground">
              <span>Elapsed: {formatTime(elapsedTime)}</span>
              {progress.percentage > 0 && (
                <span>
                  ETA: {formatTime(Math.round(elapsedTime * (100 - progress.percentage) / progress.percentage))}
                </span>
              )}
            </div>
          </>
        )}

        {!isGenerating && (
          <div className="flex items-center gap-2 text-green-600 text-sm">
            <Download className="w-4 h-4" />
            Trace file downloaded successfully
          </div>
        )}
      </CardContent>
    </Card>
  )
}