# frozen_string_literal: true

require 'zoo'

RSpec.describe Zoo::Cart do
  let(:cart) { described_class.new(Zoo::Pricing.new(:us), Zoo::Rebate.new(10)) }

  it 'adds items' do
    cart.add(Zoo::Item.new('a', 5))
    expect(cart.count).to eq(1)
  end

  it 'totals' do
    cart.add(Zoo::Item.new('a', 5, 2))
    expect(cart.subtotal).to eq(10)
  end
end

RSpec.describe Zoo::Pricing do
  it "logs" do
    p = described_class.new(:us)
    p.send(:write_log, "x")
  end

  it 'uppercases region' do
    expect(described_class.new(:us).send(:region_key)).to eq('US')
  end
end
