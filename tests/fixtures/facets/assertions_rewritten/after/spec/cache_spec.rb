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
    assert_equal 3, cache.read(:c)
    expect(cache.size).to be(2)
  end
end
