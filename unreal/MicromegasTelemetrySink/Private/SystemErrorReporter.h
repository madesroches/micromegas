#pragma once

class FSystemErrorReporter
{
public:
	FSystemErrorReporter();
	~FSystemErrorReporter();
private:
	void OnSystemError();
};
