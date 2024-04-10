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
			"HTTP",
		});

		PrivateIncludePaths.Add( "MicromegasTelemetrySink/Private/jsoncons-0.173.4" );
	}
}
