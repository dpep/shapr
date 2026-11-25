RSpec.describe Cart do
  it "sums" do
    expect(Cart.new.total([])).to eq(0)
    expect(Cart.new.total([])).to be_zero
  end
end
