//
//  MicromegasTelemetrySink/ScreenshotSender.cpp
//
#include "ScreenshotSender.h"
#include "Engine/Engine.h"
#include "Engine/GameViewportClient.h"
#include "Engine/GameViewportDelegates.h"
#include "ImageUtils.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTelemetrySink/Log.h"
#include "UnrealClient.h"
#if WITH_EDITOR
	#include "Editor.h"
#endif

FScreenshotSender::FScreenshotSender()
{
	Cmd.Reset(new FAutoConsoleCommand(
		TEXT("telemetry.screenshot"),
		TEXT("Captures the game viewport and sends it as a telemetry image"),
		FConsoleCommandDelegate::CreateRaw(this, &FScreenshotSender::OnCommand)));
}

FScreenshotSender::~FScreenshotSender()
{
	UGameViewportClient::OnScreenshotCaptured().Remove(Handle);
}

void FScreenshotSender::OnCommand()
{
	if (Handle.IsValid())
	{
		UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("telemetry.screenshot: capture already pending"));
		return;
	}

	if (GEngine && GEngine->GameViewport)
	{
		Handle = UGameViewportClient::OnScreenshotCaptured().AddRaw(this, &FScreenshotSender::OnScreenshotCaptured);
		FScreenshotRequest::RequestScreenshot(false);
		return;
	}

#if WITH_EDITOR
	if (GEditor)
	{
		FViewport* EditorViewport = GEditor->GetActiveViewport();
		if (!EditorViewport)
		{
			UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("telemetry.screenshot: no editor viewport available"));
			return;
		}
		TArray<FColor> Bitmap;
		if (!EditorViewport->ReadPixels(Bitmap))
		{
			UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("telemetry.screenshot: ReadPixels failed"));
			return;
		}
		OnScreenshotCaptured(EditorViewport->GetSizeXY().X, EditorViewport->GetSizeXY().Y, Bitmap);
		return;
	}
#endif

	UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("telemetry.screenshot: no game viewport available"));
}

void FScreenshotSender::OnScreenshotCaptured(int32 Width, int32 Height, const TArray<FColor>& Bitmap)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	UGameViewportClient::OnScreenshotCaptured().Remove(Handle);
	Handle.Reset();

	TArray<FColor> FixedBitmap = Bitmap;
	for (FColor& Pixel : FixedBitmap)
	{
		Pixel.A = 255;
	}
	TArray64<uint8> CompressedPng;
	FImageUtils::PNGCompressImageArray(Width, Height, TArrayView64<const FColor>(FixedBitmap.GetData(), FixedBitmap.Num()), CompressedPng);
	MicromegasTracing::Dispatch::SendImage(
		TEXT("screenshot"),
		TEXT("image/png"),
		CompressedPng.GetData(),
		static_cast<uint32>(CompressedPng.Num()));
}
