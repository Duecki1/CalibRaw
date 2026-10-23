#import "../../../crates/calibraw-ui/src/platform/ios_scene.m"

@interface CalibRawTestDismissal : NSObject
@property(nonatomic, copy) void (^completion)(void);
- (void)dismissViewControllerAnimated:(BOOL)animated completion:(void (^)(void))completion;
@end

@implementation CalibRawTestDismissal
- (void)dismissViewControllerAnimated:(BOOL)animated completion:(void (^)(void))completion {
    self.completion = completion;
}
@end

@interface CalibRawTestPhotoResult : NSObject
@property(nonatomic, strong) NSItemProvider *itemProvider;
@end
@implementation CalibRawTestPhotoResult
@end

static void record_result(void *context, const char *json) {
    NSMutableArray *responses = (__bridge NSMutableArray *)context;
    NSData *data = [[NSString stringWithUTF8String:json] dataUsingEncoding:NSUTF8StringEncoding];
    NSDictionary *response = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    NSCAssert(response, @"The picker must provide valid JSON");
    [responses addObject:response];
    NSString *staging = response[@"staging_directory"];
    if (staging) {
        [NSFileManager.defaultManager removeItemAtPath:staging error:nil];
    }
}

static CalibRawDocumentPickerDelegate *start_session(NSMutableArray *responses) {
    CalibRawDocumentPickerDelegate *delegate = [CalibRawDocumentPickerDelegate new];
    delegate.context = (__bridge void *)responses;
    delegate.callback = record_result;
    activeDocumentPicker = delegate;
    return delegate;
}

int main(void) {
    @autoreleasepool {
        NSMutableArray *responses = [NSMutableArray array];
        NSObject *chooser = [NSObject new];
        NSObject *photos = [NSObject new];
        CalibRawDocumentPickerDelegate *delegate = start_session(responses);
        delegate.activePresentation = (UIPresentationController *)photos;
        [delegate presentationControllerDidDismiss:(UIPresentationController *)chooser];
        NSCAssert(responses.count == 0, @"Dismissing the chooser must not cancel Photos");
        __weak CalibRawDocumentPickerDelegate *retainedDelegate = delegate;
        delegate = nil;
        NSCAssert(retainedDelegate, @"Photos must retain its callback session");

        CalibRawTestDismissal *dismissal = [CalibRawTestDismissal new];
        [retainedDelegate picker:(PHPickerViewController *)dismissal didFinishPicking:@[]];
        NSCAssert(responses.count == 0, @"Close must wait for dismissal completion");
        dismissal.completion();
        NSCAssert(responses.count == 1 && [responses[0][@"cancelled"] boolValue], @"Close must report cancellation");
        dismissal.completion();
        NSCAssert(responses.count == 1 && !activeDocumentPicker, @"Close must complete exactly once");
        dismissal.completion = nil;

        [responses removeAllObjects];
        delegate = start_session(responses);
        delegate.activePresentation = (UIPresentationController *)photos;
        [delegate presentationControllerDidDismiss:(UIPresentationController *)photos];
        [delegate presentationControllerDidDismiss:(UIPresentationController *)photos];
        NSCAssert(responses.count == 1 && !activeDocumentPicker, @"Interactive dismissal must complete exactly once");

        [responses removeAllObjects];
        delegate = start_session(responses);
        CalibRawTestPhotoResult *result = [CalibRawTestPhotoResult new];
        result.itemProvider = [NSItemProvider new];
        [delegate picker:(PHPickerViewController *)dismissal
            didFinishPicking:(NSArray<PHPickerResult *> *)@[result]];
        NSCAssert(responses.count == 0, @"Confirm must wait for dismissal completion");
        dismissal.completion();
        NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:2];
        while (!responses.count && deadline.timeIntervalSinceNow > 0) {
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        }
        NSCAssert(responses.count == 1 && [responses[0][@"errors"] count] == 1,
            @"Confirm must deliver selected-provider results");
        NSCAssert(!activeDocumentPicker, @"Confirm must release the picker session");
        puts("Picker lifecycle: chooser handoff, close, confirm, and exactly-once completion passed");
    }
    return 0;
}