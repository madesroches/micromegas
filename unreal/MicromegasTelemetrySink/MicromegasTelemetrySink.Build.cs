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
			"Json", //todo: remove
		});
	}
}
