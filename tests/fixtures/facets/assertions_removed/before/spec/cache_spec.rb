RSpec.describe Cache do
  it "evicts the oldest entry" do
    cache = Cache.new(size: 2)
    cache.write(:a, 1)
    cache.write(:b, 2)
    expect(cache.read(:a)).to eq(1)
    expect(cache.read(:b)).to eq(2)
    cache.write(:c, 3)
    expect(cache.read(:a)).to be_nil
    expect(cache.read(:b)).to eq(2)
    expect(cache.read(:c)).to eq(3)
    expect(cache.size).to eq(2)
  end
end
