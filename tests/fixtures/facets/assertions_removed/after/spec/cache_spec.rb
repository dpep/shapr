RSpec.describe Cache do
  it "evicts the oldest entry" do
    cache = Cache.new(size: 2)
    cache.write(:a, 1)
    cache.write(:b, 2)
    cache.write(:c, 3)
    expect(cache.read(:a)).to be_nil
  end
end
