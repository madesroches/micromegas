using System.IO;
using UnrealBuildTool;

public class MicromegasTelemetrySink : ModuleRules
{
	public MicromegasTelemetrySink(ReadOnlyTargetRules Target) : base(Target)
	{
		PrivateDependencyModuleNames.AddRange([
			"BuildSettings",
			"Core",
			"CoreUObject",
			"Engine", 
			"Json",
			"HTTP",
			"RenderCore",
			"RHI",
		]);

		PrivateIncludePaths.Add( Path.Combine(ModuleDirectory, "ThirdParty/jsoncons-0.173.4") );
		PrivateIncludePaths.Add( Path.Combine(ModuleDirectory, "ThirdParty") );
	}
}
