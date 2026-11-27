package com.kravtsov.kumowatch

import androidx.core.content.FileProvider

/** AndroidX asks for an app's own subclass: some devices fail with FileProvider declared directly. */
class LogsFileProvider : FileProvider(R.xml.shared_logs)
