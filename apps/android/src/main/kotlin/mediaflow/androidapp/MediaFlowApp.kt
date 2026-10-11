package mediaflow.androidapp

import android.app.Application

/** App 级容器持有（进程单例）。 */
class MediaFlowApp : Application() {
    lateinit var container: SdkContainer
        private set

    override fun onCreate() {
        super.onCreate()
        container = SdkContainer(this)
    }
}
