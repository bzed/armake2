#define ARMAKE2_TEST_NAME "armake2 test"
#define ARMAKE2_TEST_VERSION 1.0

class CfgPatches
{
	class armake2_test
	{
		units[] = {};
		weapons[] = {};
		requiredVersion = 0.1;
		requiredAddons[] =
		{
			"DZ_Data"
		};
	};
};

class CfgMods
{
	class armake2_test
	{
		dir = "armake2_test";
		picture = "";
		action = "";
		hideName = 1;
		hidePicture = 1;
		name = ARMAKE2_TEST_NAME;
		credits = "armake2 test harness";
		author = "armake2 test harness";
		authorID = "0";
		version = "1.0";
		extra = 0;
		type = "mod";

		dependencies[] = { "Game", "World", "Mission" };

		class defs
		{
			class missionScriptModule
			{
				value = "";
				files[] =
				{
					"armake2_test/scripts/5_Mission"
				};
			};
		};
	};
};
