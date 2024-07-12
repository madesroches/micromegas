using System;
using System.Collections.Generic;
using System.IO;
using UnrealBuildTool;

public class MicromegasTelemetrySink : ModuleRules
{
	public MicromegasTelemetrySink(ReadOnlyTargetRules Target) : base(Target)
	{
		PrivateDependencyModuleNames.AddRange(new string[]
		{
			"Core",
			"CoreUObject",
			"HTTP",
			"Engine",
		});

		PrivateIncludePaths.Add( Path.Combine(ModuleDirectory, "ThirdParty/jsoncons-0.173.4") );
		PrivateIncludePaths.Add( Path.Combine(ModuleDirectory, "ThirdParty") );
	}
}
