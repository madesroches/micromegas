#pragma once
//
//  MicromegasTelemetrySink/ScreenshotSender.h
//
#include "CoreMinimal.h"
#include "HAL/IConsoleManager.h"
#include "Templates/UniquePtr.h"

class FScreenshotSender
{
public:
	FScreenshotSender();
	~FScreenshotSender();

private:
	void OnCommand();
	void OnScreenshotCaptured(int32 Width, int32 Height, const TArray<FColor>& Bitmap);

	TUniquePtr<FAutoConsoleCommand> Cmd;
	FDelegateHandle Handle;
};
