// KMP Shared SDK：业务与数据逻辑（网络/SSE/DTO/分页/Repository/缓存/认证抽象）。
// 分层（决策 D-0003）：
//   mediaflow.api.*     —— 由 contracts/openapi/mediaflow.v1.yaml 构建期生成（openapi-generator），
//                          不手改、不进 lint；契约漂移由编译 + fixtures 回放测试拦截。
//   mediaflow.client.*  —— 手写 SDK 核心。

plugins {
    alias(libs.plugins.kotlin.multiplatform)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.android.library)
    alias(libs.plugins.sqldelight)
    alias(libs.plugins.detekt)
    alias(libs.plugins.ktlint)
    alias(libs.plugins.kover)
    alias(libs.plugins.dokka)
}

kotlin {
    // AGP 9 起 KMP 模块经 android 块声明 Android 目标（com.android.library 与
    // org.jetbrains.kotlin.multiplatform 已不兼容，内置 KMP 支持取代两者组合）。
    android {
        namespace = "mediaflow.kmp.sdk"
        compileSdk = 36
        minSdk = 26
    }
    jvm()
    iosArm64()
    iosSimulatorArm64()

    // 生成器输出挂载为 commonMain 源（路径不含任务依赖，编译依赖在下方显式接线；
    // 格式化豁免经根 .editorconfig 的 ktlint=disabled 按路径关闭——KMP common 层级
    // 不允许自定义 sourceSet dependsOn，故不走独立源集路线）。
    val generatedMain = layout.buildDirectory.dir("generated/openapi/src/commonMain/kotlin")

    sourceSets {
        commonMain {
            kotlin.srcDir(generatedMain)
            dependencies {
                api(libs.kotlinx.datetime)
                api(libs.kotlinx.coroutines.core)
                api(libs.kotlinx.serialization.json)
                api(libs.ktor.client.core)
                api(libs.ktor.client.content.negotiation)
                api(libs.ktor.serialization.kotlinx.json)
                api(libs.sqldelight.runtime)
                api(libs.sqldelight.coroutines)
            }
        }
        commonTest {
            dependencies {
                implementation(kotlin("test"))
                implementation(libs.kotlinx.coroutines.test)
                implementation(libs.ktor.client.mock)
            }
        }
        jvmMain {
            dependencies {
                implementation(libs.ktor.client.cio)
                implementation(libs.sqldelight.driver.sqlite)
            }
        }
        androidMain {
            dependencies {
                implementation(libs.ktor.client.okhttp)
                implementation(libs.sqldelight.driver.android)
            }
        }
        iosMain {
            dependencies {
                implementation(libs.ktor.client.darwin)
                implementation(libs.sqldelight.driver.native)
            }
        }
    }
}

// 客户端测试统一走 kotlin multiplatform test（jvm + iosSimulator），不注册 android 单测变体。

// SQLDelight：C1 引入缓存 schema 与迁移；插件先接入验证工具链（无数据库注册）。
sqldelight {
}

// ---------------------------------------------------------------------------
// 契约 → 客户端生成（决策 D-0002）
// openapi-generator 官方 gradle 插件发布停滞在 7.14（模板为 Ktor 1.x 时代），
// 因此直接依赖 7.26 fat jar，经 JavaExec 驱动同一 mainClass（OpenAPIGenerator，
// 与官方插件一致），构建期生成进 build/generated/（不提交）。
// 契约路径用模块相对路径定位（不用 rootProject，保持 configuration-cache 兼容）。
// ---------------------------------------------------------------------------
val openapiGeneratorDeps = configurations.create("openapiGeneratorDeps")

dependencies {
    openapiGeneratorDeps(libs.openapi.generator.cli)
}

val openApiGenerate =
    tasks.register<JavaExec>("openApiGenerate") {
        group = "codegen"
        description = "从 contracts/openapi/mediaflow.v1.yaml 生成 KMP 客户端（DTO + API）"
        mainClass = "org.openapitools.codegen.OpenAPIGenerator"
        classpath = openapiGeneratorDeps
        val contract = layout.projectDirectory.file("../../contracts/openapi/mediaflow.v1.yaml")
        val output = layout.buildDirectory.dir("generated/openapi")
        inputs.file(contract)
        outputs.dir(output)
        // 参数经 provider 延迟求值，配置缓存下不序列化脚本状态。
        argumentProviders.add(
            CommandLineArgumentProvider {
                listOf(
                    "generate",
                    "-g",
                    "kotlin",
                    "-i",
                    contract.asFile.absolutePath,
                    "-o",
                    output.get().asFile.absolutePath,
                    "--library=multiplatform",
                    "--additional-properties=" +
                        listOf(
                            "packageName=mediaflow.api",
                            "groupId=mediaflow",
                            "artifactId=kmp-sdk-api",
                            "artifactVersion=0.1.0",
                            "dateLibrary=kotlinx-datetime",
                        ).joinToString(","),
                )
            },
        )
        // 输出目录整删重建，保证构建期再生成的确定性（目录内含生成器自己的构建文件，不编译）。
        doFirst {
            output.get().asFile.deleteRecursively()
        }
        // 生成器 7.26 对 OpenAPI 3.1 有两类构造缺陷，这里做确定性后处理补丁（精确锚定，
        // 不做宽泛改写；两类均为主线契约冻结时的 KMP 友好构造，升级生成器后应重审）：
        //   ① 字面 null 类型 {type:'null'}（9 个事件模型的 task_id）：引用 Null 类但
        //      不生成定义 → 属性 `: Null` 改 `: kotlin.String?`（与外层 envelope 的
        //      [string,null] 演进一致），并移除幽灵 import。
        //   ② const 字符串字段 {const:...}：生成为 kotlin.Any?（无序列化器）→ 属性级
        //      `val x: kotlin.Any?` 改 `kotlin.String?`（本契约 const 全部为字符串；
        //      函数签名中的 data: kotlin.Any? 不受锚定影响）。
        doLast {
            val modelsDir = output.get().asFile.resolve("src/commonMain/kotlin/mediaflow/api/models")
            val importRegex = Regex("""(?m)^import mediaflow\.api\.models\.Null\r?\n""")
            val constPropRegex = Regex("""(val \w+: )kotlin\.Any\?""")
            modelsDir.listFiles { file -> file.extension == "kt" }?.forEach { file ->
                val text = file.readText()
                if (importRegex.containsMatchIn(text) || constPropRegex.containsMatchIn(text)) {
                    file.writeText(
                        text
                            .replace(importRegex, "")
                            .replace(": Null,", ": kotlin.String?,")
                            .replace(": Null)", ": kotlin.String?)")
                            .replace(constPropRegex, "$1kotlin.String?"),
                    )
                }
            }
        }
    }

// srcDir 挂的是路径 Provider（不含任务依赖），这里显式接线消费生成目录的任务：
// 各 KMP 目标的主编译（含 AGP 内置 KMP 命名，如 compileAndroidMain）、metadata
// 编译，以及把 commonMain 源集（含生成目录）当输入的 ktlint 检查。
kotlin.targets.configureEach {
    compilations.getByName("main").compileTaskProvider.configure {
        dependsOn(openApiGenerate)
    }
}
tasks.configureEach {
    val needsGenerated = (name.startsWith("compile") && name.endsWith("Metadata")) || name.startsWith("runKtlint")
    if (needsGenerated) {
        dependsOn(openApiGenerate)
    }
}

detekt {
    buildUponDefaultConfig = true
    allRules = false
    config.setFrom(files("$projectDir/detekt.yml"))
    // detekt 默认只扫 JVM 约定目录；KMP 源集显式列出。
    source.setFrom(
        "src/commonMain/kotlin",
        "src/commonTest/kotlin",
        "src/jvmMain/kotlin",
        "src/androidMain/kotlin",
        "src/iosMain/kotlin",
    )
}

ktlint {
    // 版本不单独钉：插件默认引擎与插件版本配套发布，避免双源漂移。
    // 生成物的格式化豁免见根 .editorconfig（ktlint=disabled 按路径）。
}

kover {
    reports {
        // 生成代码（mediaflow.api）不计入覆盖率基线；度量的是手写 SDK 核心。
        filters {
            excludes {
                packages("mediaflow.api")
            }
        }
    }
}

dokka {
    moduleName.set("MediaFlow KMP SDK")
}
