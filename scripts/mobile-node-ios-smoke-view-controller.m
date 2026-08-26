#import "ViewController.h"
#import "NodeRunner.hpp"

@interface ViewController ()
@property(nonatomic, assign) BOOL pisperSmokeStarted;
@end

@implementation ViewController

+ (NSString *)pisperSmokeToken {
    NSArray<NSString *> *arguments = NSProcessInfo.processInfo.arguments;
    NSUInteger marker = [arguments indexOfObject:@"--smoke-ui"];
    if (marker == NSNotFound || marker + 1 >= arguments.count) return nil;
    return arguments[marker + 1];
}

+ (void)writeResultForToken:(NSString *)token passed:(BOOL)passed {
    NSString *documents = [NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES) firstObject];
    NSString *result = [documents stringByAppendingPathComponent:[NSString stringWithFormat:@"result-%@.txt", token]];
    NSString *value = passed ? @"PASS\n" : @"FAIL\n";
    NSError *error = nil;
    if (![value writeToFile:result atomically:YES encoding:NSUTF8StringEncoding error:&error]) {
        NSLog(@"PISPER_IOS_RUNTIME_SMOKE_RESULT_ERROR %@", error.localizedDescription);
    }
}

- (void)viewDidAppear:(BOOL)animated {
    [super viewDidAppear:animated];
    if (self.pisperSmokeStarted) return;
    self.pisperSmokeStarted = YES;

    NSString *token = [ViewController pisperSmokeToken];
    if (token.length == 0) return;

    // UIKit 必须先完成前台启动，再在后台线程运行常驻 Node Runtime，避免触发 iOS 启动看门狗。
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSString *script = [[NSBundle mainBundle] pathForResource:@"pisper-smoke" ofType:@"mjs"];
        if (script.length == 0) {
            NSLog(@"PISPER_IOS_RUNTIME_SMOKE_SCRIPT_MISSING");
            [ViewController writeResultForToken:token passed:NO];
            return;
        }
        setenv("NODE_MOBILE_RUN_TOKEN", token.UTF8String, 1);
        int code = [NodeRunner startEngineWithArguments:@[@"node", script]];
        [ViewController writeResultForToken:token passed:(code == 0)];
        NSLog(@"PISPER_IOS_RUNTIME_SMOKE_NODE_EXIT %d", code);
    });
}

@end
